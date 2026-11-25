//! Concurrent enrollment connections are bounded per peer and in total before any stream is
//! read.
use std::io;

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::Router,
};
use tokio::{
    sync::mpsc,
    time::{Duration, timeout},
};

use super::{ENROLLMENT_GLOBAL_LIMIT, ENROLLMENT_PER_PEER_LIMIT, EnrollmentHandler};
use crate::{ENROLLMENT_ALPN, EndpointSecret};

type TestResult<T = ()> = Result<T, io::Error>;

/// Far below the handler's ten-second wait for a stream, far above one local handshake.
const PROMPT: Duration = Duration::from_secs(2);

async fn test_endpoint(provenance: &'static str) -> TestResult<Endpoint> {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance(provenance))
        .bind()
        .await
        .map_err(io::Error::other)
}

async fn connect(client: &Endpoint, router: &Router) -> TestResult<Connection> {
    client
        .connect(router.endpoint().addr(), ENROLLMENT_ALPN)
        .await
        .map_err(io::Error::other)
}

#[tokio::test]
async fn a_peer_past_its_bound_is_closed_at_once_and_others_still_connect() -> TestResult {
    // Given: a Runtime that never takes a call, and one peer holding as many connections as
    // it may, each still to open its stream.
    let (calls, _never_taken) = mpsc::channel(1);
    let router = Router::builder(test_endpoint("ma2a_enrollment_limit_server").await?)
        .accept(ENROLLMENT_ALPN, EnrollmentHandler::new(calls))
        .spawn();
    let flooding = test_endpoint("ma2a_enrollment_limit_flooding").await?;
    let mut held = Vec::new();
    for _ in 0..ENROLLMENT_PER_PEER_LIMIT {
        held.push(connect(&flooding, &router).await?);
    }

    // When
    let excess = connect(&flooding, &router).await?;
    let other = test_endpoint("ma2a_enrollment_limit_other").await?;
    let admitted = connect(&other, &router).await?;

    // Then: only the excess connection is closed, without waiting for its stream.
    assert!(timeout(PROMPT, excess.closed()).await.is_ok());
    assert!(admitted.close_reason().is_none());
    assert!(
        held.iter()
            .all(|connection| connection.close_reason().is_none())
    );
    flooding.close().await;
    other.close().await;
    router.shutdown().await.map_err(io::Error::other)?;
    Ok(())
}

#[tokio::test]
async fn a_connection_past_the_global_bound_is_closed_at_once() -> TestResult {
    // Given: every slot is held by other peers.
    let (calls, _never_taken) = mpsc::channel(1);
    let handler = EnrollmentHandler::new(calls);
    let permits = (0..ENROLLMENT_GLOBAL_LIMIT)
        .map(|_| {
            handler
                .limiter
                .try_acquire(EndpointSecret::generate().endpoint_id())
                .ok_or_else(|| io::Error::other("global enrollment permit missing"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let router = Router::builder(test_endpoint("ma2a_enrollment_global_server").await?)
        .accept(ENROLLMENT_ALPN, handler)
        .spawn();
    let client = test_endpoint("ma2a_enrollment_global_client").await?;

    // When
    let refused = connect(&client, &router).await?;

    // Then
    assert!(timeout(PROMPT, refused.closed()).await.is_ok());
    drop(permits);
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)?;
    Ok(())
}

#[tokio::test]
async fn an_answered_peer_that_keeps_its_connection_holds_no_slot() -> TestResult {
    // Given: every call is answered, and the peer reads its answer but never closes.
    let (calls, mut received) = mpsc::channel(1);
    let handler = EnrollmentHandler::new(calls);
    let limiter = handler.limiter.clone();
    let answering = tokio::spawn(async move {
        while let Some(call) = received.recv().await {
            call.respond(1, Vec::new());
        }
    });
    let router = Router::builder(test_endpoint("ma2a_enrollment_release_server").await?)
        .accept(ENROLLMENT_ALPN, handler)
        .spawn();
    let client = test_endpoint("ma2a_enrollment_release_client").await?;
    let connection = connect(&client, &router).await?;
    let (mut send, mut receive) = connection.open_bi().await.map_err(io::Error::other)?;
    send.write_all(b"request").await.map_err(io::Error::other)?;
    send.finish().map_err(io::Error::other)?;
    receive.read_to_end(1_024).await.map_err(io::Error::other)?;

    // When: the connection stays open well inside the handler's ten-second wait for it.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Then: every slot is free again.
    let permits = (0..ENROLLMENT_GLOBAL_LIMIT)
        .map(|_| limiter.try_acquire(EndpointSecret::generate().endpoint_id()))
        .collect::<Option<Vec<_>>>();
    assert!(permits.is_some());
    drop(permits);
    connection.close(0_u8.into(), b"");
    client.close().await;
    router.shutdown().await.map_err(io::Error::other)?;
    answering.abort();
    Ok(())
}
