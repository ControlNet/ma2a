use std::io;

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::Router,
};
use ma2a_core::{
    ServiceKind,
    service::{FRAME_HEADER_LEN, FrameHeader, FrameKind, ServiceError},
};
use tokio::sync::mpsc;

use super::ServiceHandler;
use crate::{SHELL_ALPN, ServiceLimits, ServiceMetrics};

type TestResult<T = ()> = Result<T, io::Error>;

async fn test_endpoint(provenance: &'static str) -> TestResult<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance(provenance))
        .bind()
        .await
        .map_err(io::Error::other)
}

async fn connect(handler: ServiceHandler) -> TestResult<(Router, Endpoint, Connection)> {
    let server = test_endpoint("ma2a_service_handler_server").await?;
    let server_addr = server.addr();
    let router = Router::builder(server).accept(SHELL_ALPN, handler).spawn();
    let client = test_endpoint("ma2a_service_handler_client").await?;
    let connection = client
        .connect(server_addr, SHELL_ALPN)
        .await
        .map_err(io::Error::other)?;
    Ok((router, client, connection))
}

/// Opens a stream and writes one byte so the peer observes it; nothing is ever read by it.
async fn announce(connection: &Connection) -> TestResult<(SendStream, RecvStream)> {
    let (mut send, receive) = connection.open_bi().await.map_err(io::Error::other)?;
    send.write_all(&[0xff]).await.map_err(io::Error::other)?;
    Ok((send, receive))
}

async fn read_reject(receive: &mut RecvStream) -> TestResult<Option<ServiceError>> {
    let mut header = [0_u8; FRAME_HEADER_LEN];
    receive
        .read_exact(&mut header)
        .await
        .map_err(io::Error::other)?;
    let header = FrameHeader::parse(header).map_err(io::Error::other)?;
    assert_eq!(header.kind(), FrameKind::Reject);
    let mut code = [0_u8; 1];
    receive
        .read_exact(&mut code)
        .await
        .map_err(io::Error::other)?;
    Ok(ServiceError::from_code(code[0]))
}

#[tokio::test]
async fn saturated_peer_is_rejected_without_queueing_its_stream() -> TestResult {
    // Given
    let metrics = ServiceMetrics::default();
    let (calls, mut received) = mpsc::channel(64);
    let handler = ServiceHandler::new(ServiceKind::Shell, Some(calls), metrics.clone());
    let (router, client, connection) = connect(handler).await?;
    let limit = ServiceLimits::of(ServiceKind::Shell).per_peer();
    let mut held = Vec::new();
    for _ in 0..limit {
        let announced = announce(&connection).await?;
        held.push((
            announced,
            received
                .recv()
                .await
                .map(super::super::ServiceCall::authorize)
                .ok_or_else(|| io::Error::other("admitted call missing"))?,
        ));
    }

    // When
    let (_send, mut receive) = announce(&connection).await?;

    // Then
    assert_eq!(
        read_reject(&mut receive).await?,
        Some(ServiceError::ConcurrencyExceeded)
    );
    assert!(received.try_recv().is_err());
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.saturated_streams, 1);
    assert_eq!(
        snapshot.authorized_streams,
        u64::try_from(limit).map_err(io::Error::other)?
    );
    assert_eq!(snapshot.active_streams, snapshot.authorized_streams);
    drop(held);
    assert_eq!(metrics.snapshot().active_streams, 0);
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)
}

#[tokio::test]
async fn runtime_rejection_never_hands_the_stream_to_runtime_code() -> TestResult {
    // Given
    let metrics = ServiceMetrics::default();
    let (calls, mut received) = mpsc::channel(1);
    let handler = ServiceHandler::new(ServiceKind::Shell, Some(calls), metrics.clone());
    let (router, client, connection) = connect(handler).await?;
    let (_send, mut receive) = announce(&connection).await?;
    let call = received
        .recv()
        .await
        .ok_or_else(|| io::Error::other("call missing"))?;
    assert_eq!(call.kind(), ServiceKind::Shell);
    assert_eq!(call.remote_endpoint_id(), client.id().into());

    // When
    call.reject(ServiceError::Unauthorized);

    // Then: its place is free before the peer has even read the refusal.
    assert_eq!(metrics.snapshot().active_streams, 0);
    assert_eq!(
        read_reject(&mut receive).await?,
        Some(ServiceError::Unauthorized)
    );
    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.rejected_streams, 1);
    assert_eq!(snapshot.authorized_streams, 0);
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)
}

#[tokio::test]
async fn a_handler_without_a_runtime_receiver_is_unavailable() -> TestResult {
    // Given
    let handler = ServiceHandler::new(ServiceKind::Shell, None, ServiceMetrics::default());
    let (router, client, connection) = connect(handler).await?;

    // When
    let (_send, mut receive) = announce(&connection).await?;

    // Then
    assert_eq!(
        read_reject(&mut receive).await?,
        Some(ServiceError::Unavailable)
    );
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)
}
