//! Per-server deterministic interleaving; never present in production builds.
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};

type Gate = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[derive(Clone, Default, Debug)]
pub(super) struct MutationTestHooks {
    pub(super) before_revoke: Arc<Mutex<Option<Gate>>>,
}

impl MutationTestHooks {
    pub(super) async fn pause_revoke(&self) {
        let gate = self.before_revoke.lock().await.take();
        if let Some((arrived, resume)) = gate {
            let _sent = arrived.send(());
            let _resumed = resume.await;
        }
    }
}
