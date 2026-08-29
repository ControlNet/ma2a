//! Real-Iroh zero-Space protocol isolation regression.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use iroh::{Endpoint, RelayMode, address_lookup::memory::MemoryLookup, endpoint::presets};
use ma2a_core::{
    AuthorizationDenied, AuthorizationEndpoints, AuthorizationRequest, RemoteOperation,
};
use ma2a_net::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS, ProtocolRole, ZERO_SPACE_ALPNS};
use ma2a_runtime::{Runtime, authorize_remote};
use ma2a_store::StoreConfig;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
const UNKNOWN_ALPN: &[u8] = b"ma2a/future-normal/99";
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PayloadAttempt {
    NegotiationRejected,
    Negotiated,
}

struct PayloadRequest<'a> {
    target: iroh::EndpointAddr,
    alpn: &'a [u8],
    payload: &'a [u8],
}

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-zero-space-isolation-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_space_rejects_normal_and_future_alpns_before_body_parsing() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let status = runtime.handle().status().await?;
    let client = test_client().await?;
    let malformed = [0xff, 0x00, 0xfe];
    let oversized = vec![0xa5; 1_048_577];

    // When
    for alpn in NORMAL_PROTOCOL_ALPNS
        .into_iter()
        .chain(std::iter::once(UNKNOWN_ALPN))
    {
        let malformed_attempt = attempt_payload(
            &client,
            PayloadRequest {
                target: status.endpoint_addr(),
                alpn,
                payload: &malformed,
            },
        )
        .await?;
        let oversized_attempt = attempt_payload(
            &client,
            PayloadRequest {
                target: status.endpoint_addr(),
                alpn,
                payload: &oversized,
            },
        )
        .await?;
        assert_eq!(malformed_attempt, PayloadAttempt::NegotiationRejected);
        assert_eq!(oversized_attempt, PayloadAttempt::NegotiationRejected);
    }

    // Then
    let enrollment = tokio::time::timeout(
        IO_TIMEOUT,
        client.connect(status.endpoint_addr(), ENROLLMENT_ALPN),
    )
    .await??;
    assert_eq!(enrollment.alpn(), ENROLLMENT_ALPN);
    let (mut send, mut receive) = enrollment.open_bi().await?;
    send.write_all(&malformed).await?;
    send.finish()?;
    let mut response = [0_u8; 3];
    receive.read_exact(&mut response).await?;
    assert_eq!(response, [1, 0, 0]);
    enrollment.close(0_u8.into(), b"");
    let _closed = enrollment.closed().await;
    client.close().await;
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_known_normal_protocol_is_fail_closed_through_one_entry_point() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let target = runtime.handle().status().await?.endpoint_id();
    let caller = ma2a_net::EndpointSecret::generate().endpoint_id();

    // When
    assert_eq!(ZERO_SPACE_ALPNS, [ENROLLMENT_ALPN]);
    for alpn in NORMAL_PROTOCOL_ALPNS {
        let role = ProtocolRole::from_alpn(alpn).ok_or("known normal ALPN was unclassified")?;
        assert!(role.requires_remote_authorization());
        let operation = role
            .operation()
            .ok_or("known normal ALPN had no closed authorization operation")?;
        let request =
            AuthorizationRequest::new(AuthorizationEndpoints::new(caller, target), operation, None);
        assert_eq!(
            authorize_remote(&request, &[]).err(),
            Some(AuthorizationDenied::ACCESS_DENIED)
        );
    }

    // Then
    assert_eq!(ProtocolRole::from_alpn(UNKNOWN_ALPN), None);
    assert_eq!(
        authorize_remote(
            &AuthorizationRequest::new(
                AuthorizationEndpoints::new(caller, target),
                RemoteOperation::UNSUPPORTED,
                None,
            ),
            &[],
        )
        .err(),
        Some(AuthorizationDenied::ACCESS_DENIED)
    );
    runtime.shutdown().await?;
    Ok(())
}

async fn test_client() -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_zero_space_isolation"))
        .bind()
        .await?)
}

async fn attempt_payload(
    client: &Endpoint,
    request: PayloadRequest<'_>,
) -> TestResult<PayloadAttempt> {
    let connection =
        tokio::time::timeout(IO_TIMEOUT, client.connect(request.target, request.alpn)).await?;
    let Ok(connection) = connection else {
        return Ok(PayloadAttempt::NegotiationRejected);
    };
    if let Ok((mut send, _receive)) = connection.open_bi().await {
        let _write = send.write_all(request.payload).await;
        let _finish = send.finish();
    }
    connection.close(1_u8.into(), b"");
    Ok(PayloadAttempt::Negotiated)
}
