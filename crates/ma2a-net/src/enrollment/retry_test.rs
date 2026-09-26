//! Real QUIC streams with deterministic response loss, never random network failure.
use super::{exchange, exchange_replayable};
use crate::ENROLLMENT_ALPN;
use iroh::{
    Endpoint, RelayMode,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use std::{
    io,
    sync::{Arc, Mutex},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Copy, Debug)]
enum FirstResponse {
    AlwaysLost,
    Lost,
    Invalid,
    Denied,
}

#[derive(Clone, Debug)]
struct ResponseLoss {
    first: FirstResponse,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl ProtocolHandler for ResponseLoss {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) = connection.accept_bi().await?;
        let request = receive.read_to_end(4096).await.map_err(io::Error::other)?;
        let attempt = {
            let mut requests = self.requests.lock().expect("request mutex");
            requests.push(request);
            requests.len()
        };
        if matches!(self.first, FirstResponse::AlwaysLost)
            || (attempt == 1 && matches!(self.first, FirstResponse::Lost))
        {
            connection.close(1_u8.into(), b"test response loss");
            return Ok(());
        }
        // Test-only framing fixtures: Core signed-bootstrap validation is a
        // separate layer. Success contains one single-byte transport frame.
        let bytes: &[u8] = match self.first {
            FirstResponse::Lost | FirstResponse::AlwaysLost => &[0, 0, 1, 0, 0, 0, 1, 42],
            FirstResponse::Invalid => &[0, 0, 0],
            FirstResponse::Denied => &[1, 0, 0],
        };
        send.write_all(bytes).await.map_err(io::Error::other)?;
        send.finish().map_err(io::Error::other)?;
        connection.closed().await;
        Ok(())
    }
}

async fn scenario(first: FirstResponse, replayable: bool) -> TestResult {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .alpns(vec![ENROLLMENT_ALPN.to_vec()])
        .bind()
        .await?;
    let address = server.addr();
    let router = Router::builder(server)
        .accept(
            ENROLLMENT_ALPN,
            ResponseLoss {
                first,
                requests: Arc::clone(&requests),
            },
        )
        .spawn();
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await?;
    let request = b"test exact redemption bytes";
    let result = if replayable {
        exchange_replayable(&client, address, request).await
    } else {
        exchange(&client, address, request).await
    };
    client.close().await;
    router.shutdown().await?;
    let requests = requests.lock().expect("request mutex");
    match first {
        FirstResponse::Lost if replayable => {
            assert_eq!(result?, (0, vec![vec![42]]));
            assert_eq!(requests.len(), 2);
        }
        FirstResponse::AlwaysLost => {
            assert!(result.is_err());
            assert_eq!(requests.len(), 2);
        }
        FirstResponse::Lost | FirstResponse::Invalid => {
            assert!(result.is_err());
            assert_eq!(requests.len(), 1);
        }
        FirstResponse::Denied => {
            assert_eq!(result?, (1, vec![]));
            assert_eq!(requests.len(), 1);
        }
    }
    assert!(requests.iter().all(|bytes| bytes == request));
    drop(requests);
    Ok(())
}

#[tokio::test]
async fn lost_redemption_response_retries_identical_bytes_once() -> TestResult {
    scenario(FirstResponse::Lost, true).await
}
#[tokio::test]
async fn ordinary_enrollment_exchange_does_not_retry_ambiguous_effects() -> TestResult {
    scenario(FirstResponse::Lost, false).await
}
#[tokio::test]
async fn malformed_bootstrap_is_not_retried() -> TestResult {
    scenario(FirstResponse::Invalid, true).await
}
#[tokio::test]
async fn explicit_enrollment_denial_is_not_retried() -> TestResult {
    scenario(FirstResponse::Denied, true).await
}

#[tokio::test]
async fn repeated_response_loss_stops_after_one_retry() -> TestResult {
    scenario(FirstResponse::AlwaysLost, true).await
}
