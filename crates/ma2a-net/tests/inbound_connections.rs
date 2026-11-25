//! One peer's inbound connections are bounded across every ALPN, not per handler.

use std::{error::Error, time::Duration};

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, presets},
};
use ma2a_net::{
    CONTROL_ALPN, ECHO_ALPN, EndpointBindOptions, EndpointSecret, PEER_CONNECTION_LIMIT,
    RuntimeEndpoint, SHELL_ALPN,
};
use tokio::sync::mpsc;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

/// Far below any handler's ten-second wait, far above one local handshake.
const PROMPT: Duration = Duration::from_secs(2);

async fn raw_client(provenance: &'static str) -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance(provenance))
        .bind()
        .await?)
}

async fn connect(
    client: &Endpoint,
    server: &RuntimeEndpoint,
    alpn: &[u8],
) -> TestResult<Connection> {
    Ok(client.connect(server.endpoint_addr(), alpn).await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peer_past_its_connection_bound_is_closed_at_once_on_any_alpn() -> TestResult {
    // Given: a Runtime with every normal ALPN open, and one peer holding as many idle Echo and
    // Shell connections as it may, none of them with a stream.
    let (enrollment, _enrollment_calls) = mpsc::channel(1);
    let (control, _control_calls) = mpsc::channel(1);
    let server = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::generate(),
        ma2a_net::SpaceAddressLookup::default(),
        EndpointBindOptions::new(enrollment, None).with_control(control, true),
    )
    .await?;
    let flooding = raw_client("ma2a_inbound_bound_flooding").await?;
    let mut held = Vec::new();
    for index in 0..PEER_CONNECTION_LIMIT {
        let alpn = if index % 2 == 0 {
            ECHO_ALPN
        } else {
            SHELL_ALPN
        };
        held.push(connect(&flooding, &server, alpn).await?);
    }
    // Let the server's accept tasks for the held connections run first.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // When: it opens one more, on yet another ALPN, and another peer connects.
    let excess = connect(&flooding, &server, CONTROL_ALPN).await?;
    let other = raw_client("ma2a_inbound_bound_other").await?;
    let admitted = connect(&other, &server, ECHO_ALPN).await?;

    // Then: only the excess connection is closed; a freed slot admits the peer again.
    tokio::time::timeout(PROMPT, excess.closed()).await?;
    assert!(admitted.close_reason().is_none());
    assert!(
        held.iter()
            .all(|connection| connection.close_reason().is_none())
    );
    let freed = held.pop().ok_or("no held connection")?;
    freed.close(0_u8.into(), b"");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let replacement = connect(&flooding, &server, ECHO_ALPN).await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), replacement.closed())
            .await
            .is_err()
    );
    flooding.close().await;
    other.close().await;
    server.shutdown().await?;
    Ok(())
}
