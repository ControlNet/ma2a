use crate::error::{RuntimeError, RuntimeErrorKind};

#[cfg(test)]
pub(super) struct TlsLoadGate {
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
    pub(super) released_in_time: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
pub(super) static TLS_LOAD_GATE: std::sync::Mutex<Option<TlsLoadGate>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
pub(super) static TLS_LOAD_GATE_TEST_LOCK: tokio::sync::Mutex<()> =
    tokio::sync::Mutex::const_new(());

pub(super) fn load_native_tls_blocking(
    tls: &ma2a_net::NativeRelayTlsConfig,
) -> Result<ma2a_net::LoadedRelayTls, RuntimeError> {
    load_native_tls_raw(tls).map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}

pub(super) fn load_native_tls_raw(
    tls: &ma2a_net::NativeRelayTlsConfig,
) -> Result<ma2a_net::LoadedRelayTls, ma2a_net::RelayTlsError> {
    #[cfg(test)]
    {
        let gate = TLS_LOAD_GATE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(gate) = gate {
            let _entered = gate.entered.send(());
            let woke = gate
                .release
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok();
            gate.released_in_time
                .store(woke, std::sync::atomic::Ordering::SeqCst);
        }
    }
    tls.load_tls()
}
