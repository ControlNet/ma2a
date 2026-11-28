use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use ma2a_store::{RelayConfiguration, RelayTransportConfiguration, StoreConfig};

use super::{TempState, TestResult};
use crate::{
    Runtime,
    actor::{
        relay_configuration::{TLS_LOAD_GATE, TLS_LOAD_GATE_TEST_LOCK, TlsLoadGate},
        relay_lifecycle_test::{disabled_configuration, enabled_configuration},
        relay_tls_load::{TLS_LOAD_COMPLETIONS, wait_for_gate_done},
    },
};

const TEST_CERT_PEM: &str = "\
-----BEGIN CERTIFICATE-----\n\
MIIBSTCB8KADAgECAhQC5MBKZ9Trm5Lu0sfanwRVCvzQADAKBggqhkjOPQQDAjAU\n\
MRIwEAYDVQQDDAlsb2NhbGhvc3QwIBcNMjAwMTAxMDAwMDAwWhgPMjEyNTAxMDEw\n\
MDAwMDBaMBQxEjAQBgNVBAMMCWxvY2FsaG9zdDBZMBMGByqGSM49AgEGCCqGSM49\n\
AwEHA0IABDREpeqcGCVpLxRtGwn78P2SvMGTmiMRCDopLALFJzeI7FnMYfz/Gyw6\n\
sb31wNUz/SUSDkySCuUYiim3zZultZGjHjAcMBoGA1UdEQQTMBGCCWxvY2FsaG9z\n\
dIcEfwAAATAKBggqhkjOPQQDAgNIADBFAiARKM+U7ZI9SpEiQaqCKcRier6IJYwK\n\
1hWFxg+1MokBWgIhAO+0UOJV0JVoEvPd9Wb/PAk938wBtU+tE7qBuppISAq+\n\
-----END CERTIFICATE-----\n";

const TEST_KEY_PEM: &str = "\
-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgAWjxWXi/raqfjSBG\n\
12uPHOOtx0cJrtRo4yZEDATqi5ahRANCAAQ0RKXqnBglaS8UbRsJ+/D9krzBk5oj\n\
EQg6KSwCxSc3iOxZzGH8/xssOrG99cDVM/0lEg5MkgrlGIopt82bpbWR\n\
-----END PRIVATE KEY-----\n";

pub(super) fn write_tls_fixture(
    state: &TempState,
) -> Result<RelayTransportConfiguration, Box<dyn std::error::Error + Send + Sync>> {
    let cert_path = state.0.join("relay.cert.pem");
    let key_path = state.0.join("relay.key.pem");
    std::fs::write(&cert_path, TEST_CERT_PEM)?;
    std::fs::write(&key_path, TEST_KEY_PEM)?;
    ma2a_store::protect_private_file(&key_path, "test private relay key")?;
    Ok(RelayTransportConfiguration::NativeTls {
        certificate_path: cert_path.display().to_string(),
        private_key_path: key_path.display().to_string(),
    })
}

pub(super) fn native_tls_config(
    space: ma2a_core::SpaceId,
    address: std::net::SocketAddr,
    transport: RelayTransportConfiguration,
) -> RelayConfiguration {
    let mut desired = enabled_configuration(vec![space]);
    desired.listener_address = Some(address.to_string());
    desired.private_relay_url = Some("https://localhost/".to_owned());
    desired.transport = Some(transport);
    desired
}

pub(super) async fn wait_for_gate(entered_rx: mpsc::Receiver<()>) -> TestResult {
    tokio::time::advance(Duration::from_secs(90)).await;
    tokio::task::yield_now().await;
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(2))).await??;
    Ok(())
}

pub(super) async fn drain_tls_completion(
    handle: &crate::RuntimeHandle,
    before: usize,
) -> TestResult {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while TLS_LOAD_COMPLETIONS.load(Ordering::SeqCst) <= before {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for background TLS load completion"
        );
        handle.status().await?;
        tokio::task::yield_now().await;
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn maintenance_native_tls_retry_does_not_stall_status_or_snapshot() -> TestResult {
    let _gate_guard = TLS_LOAD_GATE_TEST_LOCK.lock().await;
    // Given: native TLS relay configuration fails initially because the port is occupied.
    let state = TempState::new("tls-retry-nonblocking")?;
    let transport = write_tls_fixture(&state)?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("tls-retry".to_owned()).await?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = occupied.local_addr()?;
    let desired = native_tls_config(space, address, transport);
    let error = handle
        .set_relay_configuration(desired)
        .await
        .expect_err("occupied listener must fail");
    assert!(error.is_retryable_background());
    drop(occupied);

    // When: maintenance retries TLS loading while the gate holds the blocking file read.
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let released_in_time = Arc::new(AtomicBool::new(false));
    *TLS_LOAD_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TlsLoadGate {
        certificate_path: state.0.join("relay.cert.pem"),
        entered: entered_tx,
        release: release_rx,
        released_in_time: Arc::clone(&released_in_time),
        done: done_tx,
    });
    wait_for_gate(entered_rx).await?;

    // Then: status and snapshot complete without waiting for the TLS load, and convergence finishes once released.
    let status = tokio::time::timeout(Duration::from_millis(500), handle.status())
        .await
        .map_err(|_| "Runtime status stalled while maintenance loaded TLS files")??;
    assert!(status.is_ready());
    let snapshot = tokio::time::timeout(Duration::from_millis(500), handle.snapshot())
        .await
        .map_err(|_| "Runtime snapshot stalled while maintenance loaded TLS files")??;
    assert!(snapshot.revision() >= status.revision());
    let _released = release_tx.send(());
    wait_for_gate_done(done_rx).await?;

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while handle.relay_status().await?.convergence_pending {
        assert!(
            std::time::Instant::now() < deadline,
            "relay did not converge after gate release"
        );
        tokio::task::yield_now().await;
    }
    assert!(released_in_time.load(Ordering::SeqCst));
    assert_eq!(
        handle.relay_status().await?.private_listen_addr,
        Some(address)
    );
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stale_maintenance_native_tls_load_is_discarded_when_configuration_changes() -> TestResult {
    let _gate_guard = TLS_LOAD_GATE_TEST_LOCK.lock().await;
    // Given: a pending native TLS configuration enters its maintenance retry TLS load.
    let state = TempState::new("tls-retry-stale")?;
    let transport = write_tls_fixture(&state)?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("tls-stale".to_owned()).await?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = occupied.local_addr()?;
    let desired = native_tls_config(space, address, transport);
    assert!(handle.set_relay_configuration(desired).await.is_err());
    drop(occupied);

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let released_in_time = Arc::new(AtomicBool::new(false));
    *TLS_LOAD_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TlsLoadGate {
        certificate_path: state.0.join("relay.cert.pem"),
        entered: entered_tx,
        release: release_rx,
        released_in_time: Arc::clone(&released_in_time),
        done: done_tx,
    });
    wait_for_gate(entered_rx).await?;

    // When: desired configuration changes before the gated TLS load completes.
    handle
        .set_relay_configuration(disabled_configuration())
        .await?;
    let before_completions = TLS_LOAD_COMPLETIONS.load(Ordering::SeqCst);
    let _released = release_tx.send(());
    wait_for_gate_done(done_rx).await?;
    drain_tls_completion(&handle, before_completions).await?;

    // Then: the stale preloaded TLS result is ignored and the newer configuration stays applied.
    let status = handle.relay_status().await?;
    assert!(!status.convergence_pending);
    assert!(status.applied_private.is_none());
    assert!(status.private_listen_addr.is_none());
    assert!(released_in_time.load(Ordering::SeqCst));
    runtime.shutdown().await?;
    Ok(())
}
