use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use ma2a_store::{RelayTransportConfiguration, StoreConfig};

use super::{
    TempState, TestResult,
    tls_retry::{native_tls_config, wait_for_gate, write_tls_fixture},
};
use crate::{
    Runtime,
    actor::{
        relay_configuration::{TLS_LOAD_GATE, TLS_LOAD_GATE_TEST_LOCK, TlsLoadGate},
        relay_tls_load::TLS_LOAD_COMPLETIONS,
    },
};

async fn drain_tls_completion(handle: &crate::RuntimeHandle, before: usize) -> TestResult {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
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
async fn superseded_maintenance_tls_failure_at_same_paths_does_not_stop_runtime() -> TestResult {
    let _gate_guard = TLS_LOAD_GATE_TEST_LOCK.lock().await;
    let state = TempState::new("tls-gen-failure")?;
    let transport = write_tls_fixture(&state)?;
    let RelayTransportConfiguration::NativeTls {
        ref certificate_path,
        ..
    } = transport
    else {
        return Err("expected native TLS transport".into());
    };
    let cert_backup = std::fs::read(certificate_path)?;

    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("tls-gen-fail".to_owned()).await?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = occupied.local_addr()?;
    let desired = native_tls_config(space, address, transport.clone());
    let first_error = handle
        .set_relay_configuration(desired.clone())
        .await
        .expect_err("occupied listener must fail");
    assert!(first_error.is_retryable_background());

    // Remove the certificate so the maintenance load fails, and hold it at the gate.
    std::fs::remove_file(certificate_path)?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let released_in_time = Arc::new(AtomicBool::new(false));
    *TLS_LOAD_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TlsLoadGate {
        entered: entered_tx,
        release: release_rx,
        released_in_time: Arc::clone(&released_in_time),
    });
    wait_for_gate(entered_rx).await?;

    // Restore valid material and commit a reload at the identical paths while still occupied.
    std::fs::write(certificate_path, &cert_backup)?;
    let reload_error = handle
        .reconfigure_private_relay(desired)
        .await
        .expect_err("occupied listener must still defer reload");
    assert!(reload_error.is_retryable_background());

    // Drain the superseded failed load; the Runtime must stay responsive and converge once free.
    let before_completions = TLS_LOAD_COMPLETIONS.load(Ordering::SeqCst);
    let _released = release_tx.send(());
    drain_tls_completion(&handle, before_completions).await?;
    assert!(handle.status().await?.is_ready());
    assert!(released_in_time.load(Ordering::SeqCst));

    drop(occupied);
    super::tick(&handle).await;
    let status = handle.relay_status().await?;
    assert!(!status.convergence_pending);
    assert_eq!(status.private_listen_addr, Some(address));
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn superseded_maintenance_tls_success_at_same_paths_is_discarded() -> TestResult {
    let _gate_guard = TLS_LOAD_GATE_TEST_LOCK.lock().await;
    let state = TempState::new("tls-gen-success")?;
    let transport = write_tls_fixture(&state)?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("tls-gen-ok".to_owned()).await?;
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = occupied.local_addr()?;
    let desired = native_tls_config(space, address, transport);
    assert!(
        handle
            .set_relay_configuration(desired.clone())
            .await
            .is_err()
    );

    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let released_in_time = Arc::new(AtomicBool::new(false));
    *TLS_LOAD_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TlsLoadGate {
        entered: entered_tx,
        release: release_rx,
        released_in_time: Arc::clone(&released_in_time),
    });
    wait_for_gate(entered_rx).await?;

    // Commit a newer reload at the same paths while the listener is occupied, then free it
    // before releasing the superseded load so applying stale material would visibly converge.
    assert!(handle.reconfigure_private_relay(desired).await.is_err());
    drop(occupied);
    let before_completions = TLS_LOAD_COMPLETIONS.load(Ordering::SeqCst);
    let _released = release_tx.send(());
    drain_tls_completion(&handle, before_completions).await?;
    let after_stale = handle.relay_status().await?;
    assert!(after_stale.convergence_pending);
    assert!(after_stale.applied_private.is_none());
    assert!(released_in_time.load(Ordering::SeqCst));

    super::tick(&handle).await;
    let converged = handle.relay_status().await?;
    assert!(!converged.convergence_pending);
    assert_eq!(converged.private_listen_addr, Some(address));
    runtime.shutdown().await?;
    Ok(())
}
