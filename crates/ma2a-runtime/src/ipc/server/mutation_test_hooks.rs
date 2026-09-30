//! Per-server deterministic interleaving; never present in production builds.
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

type Gate = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[derive(Clone, Default, Debug)]
pub(super) struct MutationTestHooks {
    pub(super) before_revoke: Arc<Mutex<Option<Gate>>>,
    /// When set, the next stream-plane operation loses its target right after `open`.
    pub(super) lose_stream_after_open: Arc<std::sync::atomic::AtomicBool>,
}

impl MutationTestHooks {
    pub(super) fn take_stream_loss(&self) -> bool {
        self.lose_stream_after_open
            .swap(false, std::sync::atomic::Ordering::SeqCst)
    }

    pub(super) async fn pause_revoke(&self) {
        let gate = self.before_revoke.lock().await.take();
        if let Some((arrived, resume)) = gate {
            let _sent = arrived.send(());
            let _resumed = resume.await;
        }
    }
}
