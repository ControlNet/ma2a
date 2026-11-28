use crate::error::{RuntimeError, RuntimeErrorKind};

#[cfg(test)]
pub(super) struct TlsLoadGate {
    pub(super) certificate_path: std::path::PathBuf,
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
    pub(super) released_in_time: std::sync::Arc<std::sync::atomic::AtomicBool>,
    pub(super) done: std::sync::mpsc::Sender<()>,
}

#[cfg(test)]
pub(super) static TLS_LOAD_GATE: std::sync::Mutex<Option<TlsLoadGate>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(super) static TLS_LOAD_GATE_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

#[cfg(test)]
pub(super) static TLS_LOAD_COMPLETIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(super) fn load_native_tls_blocking(
    tls: &ma2a_net::NativeRelayTlsConfig,
) -> Result<ma2a_net::LoadedRelayTls, RuntimeError> {
    load_native_tls_raw(tls).map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}

pub(super) fn load_native_tls_raw(
    tls: &ma2a_net::NativeRelayTlsConfig,
) -> Result<ma2a_net::LoadedRelayTls, ma2a_net::RelayTlsError> {
    let result = tls.load_tls();
    #[cfg(test)]
    {
        let gate = {
            let mut slot = TLS_LOAD_GATE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot
                .as_ref()
                .is_some_and(|gate| gate.certificate_path == tls.certificate_path())
            {
                slot.take()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            let _entered = gate.entered.send(());
            let woke = gate
                .release
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok();
            gate.released_in_time
                .store(woke, std::sync::atomic::Ordering::SeqCst);
            let _done = gate.done.send(());
        }
    }
    result
}

#[cfg(test)]
pub(super) async fn wait_for_gate_done(
    done_rx: std::sync::mpsc::Receiver<()>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tokio::task::spawn_blocking(move || done_rx.recv_timeout(std::time::Duration::from_secs(2)))
        .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use ma2a_net::NativeRelayTlsConfig;

    use super::{
        TLS_LOAD_GATE, TLS_LOAD_GATE_TEST_LOCK, TlsLoadGate, load_native_tls_raw,
        wait_for_gate_done,
    };

    #[tokio::test]
    async fn unrelated_native_tls_load_does_not_consume_gate()
    -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let _guard = TLS_LOAD_GATE_TEST_LOCK.lock().await;
        let expected_cert = PathBuf::from("/nonexistent/expected-cert.pem");
        let expected = NativeRelayTlsConfig::new(
            expected_cert.clone(),
            PathBuf::from("/nonexistent/expected-key.pem"),
        );
        let unrelated = NativeRelayTlsConfig::new(
            PathBuf::from("/nonexistent/unrelated-cert.pem"),
            PathBuf::from("/nonexistent/unrelated-key.pem"),
        );
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let released_in_time = Arc::new(AtomicBool::new(false));
        *TLS_LOAD_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TlsLoadGate {
            certificate_path: expected_cert,
            entered: entered_tx,
            release: release_rx,
            released_in_time: Arc::clone(&released_in_time),
            done: done_tx,
        });

        let unrelated_result = tokio::time::timeout(
            Duration::from_millis(300),
            tokio::task::spawn_blocking(move || load_native_tls_raw(&unrelated)),
        )
        .await
        .map_err(|_| "unrelated native TLS load consumed the armed gate")??;
        assert!(unrelated_result.is_err());
        assert!(
            entered_rx.try_recv().is_err(),
            "unrelated native TLS load fired the armed gate"
        );

        let expected_task = tokio::task::spawn_blocking(move || load_native_tls_raw(&expected));
        tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(2)))
            .await??;
        let _released = release_tx.send(());
        wait_for_gate_done(done_rx).await?;
        assert!(expected_task.await?.is_err());
        assert!(released_in_time.load(Ordering::SeqCst));
        Ok(())
    }
}
