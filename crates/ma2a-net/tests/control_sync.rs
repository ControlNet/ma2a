//! Bounded authenticated control transport tests.

use std::error::Error;

use ma2a_net::{
    ControlCall, EndpointBindOptions, EndpointSecret, RuntimeEndpoint, select_peer_window,
};
use tokio::sync::mpsc;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[test]
fn peer_window_is_bounded_and_rotates_fairly() -> TestResult {
    // Given
    let local = EndpointSecret::parse(&[0x10; 32])?.endpoint_id();
    let peers = (0x20..0x26)
        .map(|seed| EndpointSecret::parse(&[seed; 32]).map(|secret| secret.endpoint_id()))
        .collect::<Result<Vec<_>, _>>()?;

    // When
    let first = select_peer_window(local, &peers, 0);
    let second = select_peer_window(local, &peers, 4);

    // Then
    assert_eq!(first.len(), 4);
    assert_eq!(second.len(), 4);
    assert_ne!(first, second);
    assert!(first.iter().all(|peer| *peer != local));
    Ok(())
}

#[test]
fn peer_window_deduplicates_inputs_and_never_exceeds_four() -> TestResult {
    // Given
    let local = EndpointSecret::parse(&[0x11; 32])?.endpoint_id();
    let peer_a = EndpointSecret::parse(&[0x21; 32])?.endpoint_id();
    let peer_b = EndpointSecret::parse(&[0x22; 32])?.endpoint_id();
    let peer_c = EndpointSecret::parse(&[0x23; 32])?.endpoint_id();
    let peer_d = EndpointSecret::parse(&[0x24; 32])?.endpoint_id();
    let peer_e = EndpointSecret::parse(&[0x25; 32])?.endpoint_id();
    let peers = [local, peer_a, peer_a, peer_b, peer_c, peer_d, peer_e];

    // When
    let selected = select_peer_window(local, &peers, 0);

    // Then
    assert_eq!(selected.len(), 4);
    assert_eq!(
        selected
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert!(!selected.contains(&local));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_handler_captures_tls_identity_and_preserves_bytes() -> TestResult {
    // Given
    let (server_enrollment, _server_enrollment_rx) = mpsc::channel(1);
    let (server_control, mut server_control_rx) = mpsc::channel::<ControlCall>(1);
    let server = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::parse(&[0x31; 32])?,
        ma2a_net::SpaceAddressLookup::default(),
        EndpointBindOptions::new(server_enrollment, None).with_control(server_control, true),
    )
    .await?;
    let (client_enrollment, _client_enrollment_rx) = mpsc::channel(1);
    let client =
        RuntimeEndpoint::bind(EndpointSecret::parse(&[0x32; 32])?, client_enrollment, None).await?;
    let expected_client = client.endpoint_id();
    let request = b"bounded-control-request".to_vec();
    let response = b"bounded-control-response".to_vec();

    // When
    let exchange = client.exchange_control(server.endpoint_addr(), &request);
    let serve = async {
        let call = server_control_rx
            .recv()
            .await
            .ok_or("missing control call")?;
        assert_eq!(call.remote_endpoint_id(), expected_client);
        let authorized = call
            .authorize()
            .ok_or("control admission receiver missing")?;
        let (received, responder) = authorized
            .request()
            .await
            .map_err(|rejection| format!("control request rejected: {rejection:?}"))?;
        assert_eq!(received, request);
        responder.respond(Ok(response.clone()));
        Ok::<(), Box<dyn Error + Send + Sync>>(())
    };
    let (received, handled) = tokio::join!(exchange, serve);

    // Then
    handled?;
    assert_eq!(received?, response);
    client.shutdown().await?;
    server.shutdown().await?;
    Ok(())
}
