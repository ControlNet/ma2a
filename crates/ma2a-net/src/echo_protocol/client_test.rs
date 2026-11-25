//! The Echo client accepts only a response frame of the documented shape.
use std::io;

use iroh::{
    Endpoint, RelayMode,
    address_lookup::memory::MemoryLookup,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use ma2a_core::{EchoError, EndpointId};

use super::{ECHO_ALPN, client::EchoClient};
use crate::{ConnectionManager, EndpointSecret};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Answers every stream with fixed response bytes, whatever it was asked.
#[derive(Clone, Debug)]
struct RawResponder(Vec<u8>);

impl ProtocolHandler for RawResponder {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) = connection.accept_bi().await?;
        receive.read_to_end(8_192).await.map_err(io::Error::other)?;
        send.write_all(&self.0).await.map_err(io::Error::other)?;
        send.finish().map_err(io::Error::other)?;
        connection.closed().await;
        Ok(())
    }
}

/// An unauthorized frame: status 3, `responder`, `duration_ms`, declared body length
/// `length`, then `rest`.
fn refusal(responder: EndpointId, (duration_ms, length): (u16, u16), rest: &[u8]) -> Vec<u8> {
    let mut bytes = vec![3];
    bytes.extend_from_slice(responder.as_bytes());
    bytes.extend_from_slice(&duration_ms.to_be_bytes());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(rest);
    bytes
}

/// Runs one Echo exchange against a responder that answers with `response(its own id)`.
async fn exchange(response: impl FnOnce(EndpointId) -> Vec<u8>) -> TestResult<EchoError> {
    let server = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await?;
    let target = EndpointId::from(server.id());
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(server.addr());
    let router = Router::builder(server)
        .accept(ECHO_ALPN, RawResponder(response(target)))
        .spawn();
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(lookup)
        .bind()
        .await?;
    let result = EchoClient::new(ConnectionManager::new(client.clone()))
        .exchange(target, b"request")
        .await;
    client.close().await;
    router.shutdown().await?;
    result.err().ok_or_else(|| "the exchange succeeded".into())
}

#[tokio::test]
async fn a_well_formed_refusal_is_reported_as_sent() -> TestResult {
    let error = exchange(|responder| refusal(responder, (0, 0), b"")).await?;
    assert_eq!(error, EchoError::Unauthorized);
    Ok(())
}

#[tokio::test]
async fn a_refusal_with_a_body_is_invalid() -> TestResult {
    let error = exchange(|responder| refusal(responder, (0, 4), b"junk")).await?;
    assert_eq!(error, EchoError::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn bytes_after_the_frame_are_invalid() -> TestResult {
    let error = exchange(|responder| refusal(responder, (0, 0), b"x")).await?;
    assert_eq!(error, EchoError::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn a_duration_past_the_saturation_point_is_invalid() -> TestResult {
    let duration = ma2a_core::MAX_ECHO_DURATION_MS + 1;
    let error = exchange(|responder| refusal(responder, (duration, 0), b"")).await?;
    assert_eq!(error, EchoError::InvalidInput);
    Ok(())
}

#[tokio::test]
async fn a_refusal_from_another_responder_is_invalid() -> TestResult {
    let other = EndpointSecret::generate().endpoint_id();
    let error = exchange(|_| refusal(other, (0, 0), b"")).await?;
    assert_eq!(error, EchoError::InvalidInput);
    Ok(())
}
