use std::{error::Error, sync::Arc};

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use tokio::time::{Duration, timeout};

use super::exchange;
use crate::{ENROLLMENT_ALPN, EndpointSecret, RuntimeEndpoint};

type TestError = Box<dyn Error + Send + Sync>;
type TestResult<T = ()> = Result<T, TestError>;

#[derive(Clone, Debug)]
struct RawResponseHandler {
    response: Arc<[u8]>,
}

impl ProtocolHandler for RawResponseHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) = connection.accept_bi().await?;
        receive
            .read_to_end(4_096)
            .await
            .map_err(std::io::Error::other)?;
        send.write_all(&self.response)
            .await
            .map_err(std::io::Error::other)?;
        send.finish().map_err(std::io::Error::other)?;
        let _closed = timeout(Duration::from_secs(5), connection.closed()).await;
        Ok(())
    }
}

#[tokio::test]
async fn success_count_zero_is_rejected() -> TestResult {
    // Given
    let response = [0, 0, 0];

    // When
    let result = exchange_raw_response(&response).await?;

    // Then
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn success_count_above_protocol_maximum_is_rejected() -> TestResult {
    // Given
    let response = [0, 0, 33];

    // When
    let result = exchange_raw_response(&response).await?;

    // Then
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn denial_with_nonzero_count_is_rejected() -> TestResult {
    // Given
    let page = vec![0xA5; 65_534];
    let mut response = vec![1, 0, 1, 0, 0, 0xFF, 0xFE];
    response.extend_from_slice(&page);

    // When
    let result = exchange_raw_response(&response).await?;

    // Then
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn extra_frame_after_declared_count_is_rejected() -> TestResult {
    // Given
    let extra_page = vec![0x5A; 65_529];
    let mut response = vec![0, 0, 1, 0, 0, 0, 1, 0x11, 0, 0, 0xFF, 0xF9];
    response.extend_from_slice(&extra_page);

    // When
    let result = exchange_raw_response(&response).await?;

    // Then
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn success_response_writes_explicit_count_before_frames() -> TestResult {
    // Given
    let response = raw_handler_response(0, vec![b"page".to_vec()]).await?;

    // When
    let expected = [0, 0, 1, 0, 0, 0, 4, b'p', b'a', b'g', b'e'];

    // Then
    assert_eq!(response, expected);
    Ok(())
}

#[tokio::test]
async fn denial_response_writes_zero_count() -> TestResult {
    // Given
    let response = raw_handler_response(7, Vec::new()).await?;

    // When
    let expected = [7, 0, 0];

    // Then
    assert_eq!(response, expected);
    Ok(())
}

async fn exchange_raw_response(
    response: &[u8],
) -> TestResult<Result<(u8, Vec<Vec<u8>>), super::NetError>> {
    let server = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_enrollment_test_server"))
        .alpns(vec![ENROLLMENT_ALPN.to_vec()])
        .bind()
        .await?;
    let server_addr = server.addr();
    let router = Router::builder(server)
        .accept(
            ENROLLMENT_ALPN,
            RawResponseHandler {
                response: Arc::from(response),
            },
        )
        .spawn();
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_enrollment_test_client"))
        .bind()
        .await?;
    let result = timeout(
        Duration::from_secs(5),
        exchange(&client, server_addr, b"request"),
    )
    .await?;
    client.close().await;
    router.shutdown().await?;
    Ok(result)
}

async fn raw_handler_response(status: u8, pages: Vec<Vec<u8>>) -> TestResult<Vec<u8>> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let server = RuntimeEndpoint::bind(EndpointSecret::generate(), sender, None).await?;
    let server_addr = server.endpoint_addr();
    let responder = tokio::spawn(async move {
        let call = receiver.recv().await.ok_or("enrollment call missing")?;
        call.respond(status, pages);
        Ok::<(), TestError>(())
    });
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_enrollment_raw_client"))
        .bind()
        .await?;
    let connection = client.connect(server_addr, ENROLLMENT_ALPN).await?;
    let (mut send, mut receive) = connection.open_bi().await?;
    send.write_all(b"request").await?;
    send.finish()?;
    let response = receive.read_to_end(1_000_000).await?;
    connection.close(0_u8.into(), b"");
    responder.await??;
    client.close().await;
    server.shutdown().await?;
    Ok(response)
}
