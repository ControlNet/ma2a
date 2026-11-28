use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use ma2a_core::SpaceId;
use ma2a_store::StoreConfig;
use serde_json::{Value, json};

use super::{
    TempState, TestResult,
    tls_retry::{drain_tls_completion, native_tls_config, wait_for_gate, write_tls_fixture},
};
use crate::{
    Runtime,
    actor::{
        relay_configuration::{TLS_LOAD_GATE, TLS_LOAD_GATE_TEST_LOCK, TlsLoadGate},
        relay_lifecycle_test::disabled_configuration,
        relay_tls_load::{TLS_LOAD_COMPLETIONS, wait_for_gate_done},
    },
};

fn control_round_count(snapshot: &Value) -> usize {
    snapshot
        .get("control_rounds")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[tokio::test(start_paused = true)]
async fn private_relay_running_convergence_advances_view_sequence_at_equal_revision() -> TestResult
{
    let _gate_guard = TLS_LOAD_GATE_TEST_LOCK.lock().await;
    let state = TempState::new("relay-view-sequence")?;
    let transport = write_tls_fixture(&state)?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    // Wait for the startup control round to finish before taking baseline counts.
    handle.sync_control().await?;

    let unjoined_space = SpaceId::derive(b"unjoined-private-relay-view-sequence");
    let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
    let address = occupied.local_addr()?;
    let desired = native_tls_config(unjoined_space, address, transport);
    let error = handle
        .set_relay_configuration(desired)
        .await
        .expect_err("occupied listener must defer convergence");
    assert!(error.is_retryable_background());
    drop(occupied);

    let rounds_before_tick = control_round_count(&handle.snapshot().await?.to_value());
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

    // Let the periodic empty control round finish while the TLS load stays parked at the gate.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while control_round_count(&handle.snapshot().await?.to_value()) <= rounds_before_tick {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for periodic control round"
        );
        tokio::task::yield_now().await;
    }

    let before_snapshot = handle.snapshot().await?;
    let before_value = before_snapshot.to_value();
    let before_stamp = handle.snapshot_stamp().await?.to_value();
    assert_eq!(
        before_value.pointer("/observed_relay_state/private_relay_provider_running"),
        Some(&json!(false))
    );
    let before_revision = before_snapshot.revision();
    let before_view = before_snapshot.view_sequence();
    let before_rounds = control_round_count(&before_value);
    assert_eq!(before_stamp.get("view_sequence"), Some(&json!(before_view)));

    // Release the TLS load so finish_relay_tls_load starts the listener without any control round
    // or Store revision change.
    let before_completions = TLS_LOAD_COMPLETIONS.load(Ordering::SeqCst);
    let _released = release_tx.send(());
    wait_for_gate_done(done_rx).await?;
    drain_tls_completion(&handle, before_completions).await?;

    let after_snapshot = handle.snapshot().await?;
    let after_value = after_snapshot.to_value();
    let after_stamp = handle.snapshot_stamp().await?.to_value();
    assert_eq!(after_snapshot.revision(), before_revision);
    assert_eq!(control_round_count(&after_value), before_rounds);
    assert_eq!(
        after_value.pointer("/observed_relay_state/private_relay_provider_running"),
        Some(&json!(true))
    );
    assert!(
        after_snapshot.view_sequence() > before_view,
        "starting private relay did not advance view_sequence ({before_view} -> {})",
        after_snapshot.view_sequence()
    );
    assert_eq!(
        after_stamp.get("view_sequence"),
        Some(&json!(after_snapshot.view_sequence()))
    );
    assert!(released_in_time.load(Ordering::SeqCst));

    // Stopping the running listener also advances view_sequence.
    let running_view = after_snapshot.view_sequence();
    handle
        .set_relay_configuration(disabled_configuration())
        .await?;
    let stopped_snapshot = handle.snapshot().await?;
    assert_eq!(
        stopped_snapshot
            .to_value()
            .pointer("/observed_relay_state/private_relay_provider_running"),
        Some(&json!(false))
    );
    assert!(
        stopped_snapshot.view_sequence() > running_view,
        "stopping private relay did not advance view_sequence ({running_view} -> {})",
        stopped_snapshot.view_sequence()
    );
    runtime.shutdown().await?;
    Ok(())
}
