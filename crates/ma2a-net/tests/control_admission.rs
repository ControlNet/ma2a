//! Inbound control authorization and concurrency admission coverage.

use std::{error::Error, io, time::Duration};

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, RecvStream, SendStream, presets},
};
use ma2a_core::MAX_CONTROL_BATCH_BYTES;
use ma2a_net::{
    CONTROL_ALPN, CONTROL_PER_PEER_LIMIT, ControlCall, ControlMetrics, ControlRejection,
    EndpointBindOptions, EndpointSecret, RuntimeEndpoint,
};
use tokio::sync::mpsc;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

struct ControlStream {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
}

async fn raw_client(provenance: &'static str) -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance(provenance))
        .bind()
        .await?)
}

async fn server(capacity: usize) -> TestResult<(RuntimeEndpoint, mpsc::Receiver<ControlCall>)> {
    let (enrollment, _enrollment_rx) = mpsc::channel(1);
    let (control, calls) = mpsc::channel(capacity);
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        EndpointSecret::generate(),
        ma2a_net::SpaceAddressLookup::default(),
        EndpointBindOptions::new(enrollment, None).with_control(control, true),
    )
    .await?;
    Ok((endpoint, calls))
}

async fn open_control(client: &Endpoint, server: &RuntimeEndpoint) -> TestResult<ControlStream> {
    let connection = client.connect(server.endpoint_addr(), CONTROL_ALPN).await?;
    let (send, receive) = connection.open_bi().await?;
    Ok(ControlStream {
        connection,
        send,
        receive,
    })
}

async fn wait_for_active(metrics: &ControlMetrics, expected: u64) -> TestResult {
    tokio::time::timeout(Duration::from_secs(5), async {
        while metrics.snapshot().active_streams != expected {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    Ok(())
}

async fn read_response(stream: &mut ControlStream) -> TestResult<u8> {
    let mut status = [0_u8; 1];
    stream.receive.read_exact(&mut status).await?;
    let mut length = [0_u8; 4];
    stream.receive.read_exact(&mut length).await?;
    assert_eq!(u32::from_be_bytes(length), 0);
    stream.connection.close(0_u8.into(), b"");
    Ok(status[0])
}

async fn hold_call(
    client: &Endpoint,
    server: &RuntimeEndpoint,
    calls: &mut mpsc::Receiver<ControlCall>,
) -> TestResult<(ControlStream, ControlCall)> {
    let mut stream = open_control(client, server).await?;
    stream.send.write_all(&[0]).await?;
    let call = calls.recv().await.ok_or("missing control call")?;
    Ok((stream, call))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthorized_peer_is_rejected_before_body_read() -> TestResult {
    // Given
    let (server, mut calls) = server(1).await?;
    let metrics = server.control_metrics();
    let client = raw_client("ma2a_control_admission_unauthorized").await?;
    let (mut stream, call) = hold_call(&client, &server, &mut calls).await?;
    assert_eq!(metrics.snapshot().body_reads, 0);

    // When
    call.reject(ControlRejection::Unauthorized);
    let status = read_response(&mut stream).await?;
    wait_for_active(&metrics, 0).await?;

    // Then
    assert_eq!(status, 1);
    assert_eq!(metrics.snapshot().body_reads, 0);
    client.close().await;
    server.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_peer_cap_rejects_maximum_request_before_body_read() -> TestResult {
    // Given
    let (server, mut calls) = server(CONTROL_PER_PEER_LIMIT + 1).await?;
    let metrics = server.control_metrics();
    let client = raw_client("ma2a_control_admission_same_peer").await?;
    let mut held = Vec::new();
    for _ in 0..CONTROL_PER_PEER_LIMIT {
        held.push(hold_call(&client, &server, &mut calls).await?);
    }
    wait_for_active(&metrics, u64::try_from(CONTROL_PER_PEER_LIMIT)?).await?;
    let excess = open_control(&client, &server).await?;

    // When
    let result = tokio::spawn(send_maximum_request(excess));
    let result = tokio::time::timeout(Duration::from_secs(5), result).await??;

    // Then
    assert!(result.is_err());
    assert_eq!(metrics.snapshot().body_reads, 0);
    assert!(calls.try_recv().is_err());
    for (mut stream, call) in held {
        call.reject(ControlRejection::Unauthorized);
        assert_eq!(read_response(&mut stream).await?, 1);
    }
    wait_for_active(&metrics, 0).await?;
    client.close().await;
    server.shutdown().await?;
    Ok(())
}

async fn send_maximum_request(mut stream: ControlStream) -> Result<(), io::Error> {
    stream
        .send
        .write_all(&vec![0xA5; MAX_CONTROL_BATCH_BYTES])
        .await
        .map_err(io::Error::other)?;
    stream.send.finish().map_err(io::Error::other)?;
    let mut status = [0_u8; 1];
    stream
        .receive
        .read_exact(&mut status)
        .await
        .map_err(io::Error::other)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permits_are_released_after_rejection_and_handler_completion() -> TestResult {
    // Given
    let (server, mut calls) = server(CONTROL_PER_PEER_LIMIT + 2).await?;
    let metrics = server.control_metrics();
    let client = raw_client("ma2a_control_admission_release").await?;
    let (mut rejected_stream, rejected_call) = hold_call(&client, &server, &mut calls).await?;
    let (mut completed_stream, completed_call) = hold_call(&client, &server, &mut calls).await?;
    wait_for_active(&metrics, u64::try_from(CONTROL_PER_PEER_LIMIT)?).await?;

    // When
    rejected_call.reject(ControlRejection::Unauthorized);
    assert_eq!(read_response(&mut rejected_stream).await?, 1);
    wait_for_active(&metrics, 1).await?;
    let (mut replacement_stream, replacement_call) =
        hold_call(&client, &server, &mut calls).await?;
    replacement_call.reject(ControlRejection::Unauthorized);
    assert_eq!(read_response(&mut replacement_stream).await?, 1);
    wait_for_active(&metrics, 1).await?;
    let authorized = completed_call
        .authorize()
        .ok_or("control admission receiver missing")?;
    completed_stream.send.write_all(b"request").await?;
    completed_stream.send.finish()?;
    let (request, responder) = authorized
        .request()
        .await
        .map_err(|rejection| format!("control request rejected: {rejection:?}"))?;
    assert_eq!(request, b"\0request");
    responder.respond(Err(ControlRejection::Invalid));
    assert_eq!(read_response(&mut completed_stream).await?, 2);
    wait_for_active(&metrics, 0).await?;

    // Then
    let (mut final_stream, final_call) = hold_call(&client, &server, &mut calls).await?;
    final_call.reject(ControlRejection::Unauthorized);
    assert_eq!(read_response(&mut final_stream).await?, 1);
    wait_for_active(&metrics, 0).await?;
    assert_eq!(metrics.snapshot().body_reads, 1);
    client.close().await;
    server.shutdown().await?;
    Ok(())
}
