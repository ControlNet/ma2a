use std::io;

use iroh::{
    Endpoint, RelayMode, address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router,
};
use tokio::sync::mpsc;

use super::{ControlHandler, ControlMetrics};
use crate::{CONTROL_ALPN, CONTROL_GLOBAL_LIMIT, EndpointSecret};

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

#[tokio::test]
async fn global_limit_exhaustion_closes_stream_before_body_read() -> TestResult {
    // Given
    let server = test_endpoint("ma2a_control_global_limit_server").await?;
    let (calls, _received_calls) = mpsc::channel(1);
    let handler = ControlHandler::new(Some(calls), ControlMetrics::default());
    let permits = (0..CONTROL_GLOBAL_LIMIT)
        .map(|_| {
            handler
                .limiter
                .try_acquire(EndpointSecret::generate().endpoint_id())
                .ok_or_else(|| io::Error::other("global control permit missing"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let router = Router::builder(server)
        .accept(CONTROL_ALPN, handler)
        .spawn();
    let client = test_endpoint("ma2a_control_global_limit_client").await?;
    let connection = client
        .connect(router.endpoint().addr(), CONTROL_ALPN)
        .await
        .map_err(io::Error::other)?;
    let (mut send, mut receive) = connection.open_bi().await.map_err(io::Error::other)?;

    // When
    send.finish().map_err(io::Error::other)?;
    let mut status = [0_u8; 1];
    let result = receive.read_exact(&mut status).await;

    // Then
    assert!(result.is_err());
    drop(permits);
    connection.close(0_u8.into(), b"");
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)?;
    Ok(())
}
