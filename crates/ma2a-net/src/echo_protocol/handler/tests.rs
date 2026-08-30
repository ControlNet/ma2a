use std::io;

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::Router,
};
use ma2a_core::EchoError;
use tokio::sync::{mpsc, oneshot};

use super::{ECHO_DEADLINE, ECHO_PROCESS_DEADLINE, EchoHandler};
use crate::{ECHO_ALPN, ECHO_GLOBAL_LIMIT, EchoMetrics, EndpointSecret, echo_protocol::frame};

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

async fn connect_handler(
    server: Endpoint,
    handler: EchoHandler,
) -> TestResult<(Router, Endpoint, Connection)> {
    let server_addr = server.addr();
    let router = Router::builder(server).accept(ECHO_ALPN, handler).spawn();
    let client = test_endpoint("ma2a_echo_handler_client").await?;
    let connection = client
        .connect(server_addr, ECHO_ALPN)
        .await
        .map_err(io::Error::other)?;
    Ok((router, client, connection))
}

async fn shutdown(router: Router, client: Endpoint) -> TestResult {
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn processing_deadline_emits_timed_out_response_frame() -> TestResult {
    // Given
    let server = test_endpoint("ma2a_echo_timeout_server").await?;
    let local_endpoint_id = server.id().into();
    let (calls, mut received_calls) = mpsc::channel(1);
    let handler = EchoHandler::new(local_endpoint_id, Some(calls), EchoMetrics::default());
    let (router, client, connection) = connect_handler(server, handler).await?;
    let (processing_started, processing_ready) = oneshot::channel();
    let runtime = tokio::spawn(async move {
        let call = received_calls
            .recv()
            .await
            .ok_or_else(|| io::Error::other("Echo call missing"))?;
        let authorized = call
            .authorize()
            .ok_or_else(|| io::Error::other("Echo admission receiver missing"))?;
        let (_request, _responder) = authorized.request().await.map_err(io::Error::other)?;
        processing_started
            .send(())
            .map_err(|()| io::Error::other("processing observer missing"))?;
        std::future::pending::<()>().await;
        Ok::<(), io::Error>(())
    });
    let (mut send, mut receive) = connection.open_bi().await.map_err(io::Error::other)?;
    send.write_all(b"request").await.map_err(io::Error::other)?;
    send.finish().map_err(io::Error::other)?;
    processing_ready.await.map_err(io::Error::other)?;
    let response = tokio::spawn(async move { frame::read(&mut receive).await });
    let started = tokio::time::Instant::now();

    // When
    tokio::time::advance(ECHO_PROCESS_DEADLINE).await;
    tokio::task::yield_now().await;
    let response = response
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)?;

    // Then
    assert!(matches!(response.result, Err(EchoError::TimedOut)));
    assert!(started.elapsed() <= ECHO_DEADLINE);
    runtime.abort();
    let _cancelled = runtime.await;
    connection.close(0_u8.into(), b"");
    shutdown(router, client).await?;
    Ok(())
}

#[tokio::test]
async fn global_limit_exhaustion_emits_concurrency_exceeded_response_frame() -> TestResult {
    // Given
    let server = test_endpoint("ma2a_echo_global_limit_server").await?;
    let handler = EchoHandler::new(server.id().into(), None, EchoMetrics::default());
    let permits = (0..ECHO_GLOBAL_LIMIT)
        .map(|_| {
            handler
                .limiter
                .try_acquire(EndpointSecret::generate().endpoint_id())
                .ok_or_else(|| io::Error::other("global Echo permit missing"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (router, client, connection) = connect_handler(server, handler).await?;
    let (mut send, mut receive) = connection.open_bi().await.map_err(io::Error::other)?;

    // When
    send.finish().map_err(io::Error::other)?;
    let response = frame::read(&mut receive).await.map_err(io::Error::other)?;

    // Then
    assert!(matches!(
        response.result,
        Err(EchoError::ConcurrencyExceeded)
    ));
    drop(permits);
    connection.close(0_u8.into(), b"");
    shutdown(router, client).await?;
    Ok(())
}
