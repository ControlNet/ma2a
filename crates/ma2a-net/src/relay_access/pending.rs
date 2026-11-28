use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh_base::EndpointId as IrohEndpointId;
use iroh_relay::server::{ConnectionId, clients::Clients};
use ma2a_core::{EndpointId, SpaceAuthorizationView};
use tokio::sync::{Notify, oneshot};

use super::{DisconnectAttempt, DisconnectClients, PrivateRelayAccess};

pub(super) const PENDING_DISCONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(test)]
type WorkerGate = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct WorkerGates {
    pub(crate) after_batch: Mutex<Option<WorkerGate>>,
    pub(crate) before_remove: Mutex<Option<WorkerGate>>,
}

#[cfg(test)]
impl WorkerGates {
    pub(crate) fn arm(
        slot: &Mutex<Option<WorkerGate>>,
    ) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        if let Ok(mut guard) = slot.lock() {
            *guard = Some((entered_tx, release_rx));
        }
        (entered_rx, release_tx)
    }

    async fn wait(slot: &Mutex<Option<WorkerGate>>) {
        let gate = slot.lock().ok().and_then(|mut guard| guard.take());
        if let Some((entered, release)) = gate {
            let _entered = entered.send(());
            let _released = release.await;
        }
    }
}

#[derive(Debug)]
pub(crate) struct PendingDisconnects {
    pending: Arc<Mutex<HashMap<(IrohEndpointId, ConnectionId), u64>>>,
    notify: Arc<Notify>,
    #[cfg(test)]
    gates: Arc<WorkerGates>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PendingDisconnects {
    pub(crate) fn spawn(access: PrivateRelayAccess, clients: Clients) -> Self {
        Self::spawn_with(access, clients)
    }

    pub(crate) fn spawn_with<C: DisconnectClients>(access: PrivateRelayAccess, clients: C) -> Self {
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let notify = Arc::new(Notify::new());
        #[cfg(test)]
        let gates = Arc::new(WorkerGates::default());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(
            PendingDisconnectWorker {
                access,
                clients,
                pending: Arc::clone(&pending),
                notify: Arc::clone(&notify),
                #[cfg(test)]
                gates: Arc::clone(&gates),
                shutdown_rx,
            }
            .run(),
        );
        Self {
            pending,
            notify,
            #[cfg(test)]
            gates,
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        }
    }

    #[cfg(test)]
    pub(crate) fn gates(&self) -> &WorkerGates {
        &self.gates
    }

    fn reconcile(
        &self,
        access: &PrivateRelayAccess,
        (generation, deferred): (u64, Vec<(IrohEndpointId, ConnectionId)>),
    ) {
        let mut notify = false;
        if let Ok(mut pending) = self.pending.lock()
            && let Ok(state) = access.state.lock()
        {
            pending.retain(|&(endpoint_id, connection_id), queued_generation| {
                *queued_generation <= state.generation
                    && state.connections.contains(&(endpoint_id, connection_id))
                    && !state.allowed.contains(&EndpointId::from(endpoint_id))
            });
            if state.generation == generation {
                for connection in deferred {
                    if state.connections.contains(&connection)
                        && !state.allowed.contains(&EndpointId::from(connection.0))
                    {
                        notify |= pending.insert(connection, generation) != Some(generation);
                    }
                }
            }
        }
        if notify {
            self.notify.notify_one();
        }
    }

    pub(crate) async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _sent = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _joined = task.await;
        }
    }
}

impl Drop for PendingDisconnects {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _sent = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct PendingDisconnectWorker<C> {
    access: PrivateRelayAccess,
    clients: C,
    pending: Arc<Mutex<HashMap<(IrohEndpointId, ConnectionId), u64>>>,
    notify: Arc<Notify>,
    #[cfg(test)]
    gates: Arc<WorkerGates>,
    shutdown_rx: oneshot::Receiver<()>,
}

impl<C: DisconnectClients> PendingDisconnectWorker<C> {
    async fn run(mut self) {
        loop {
            let batch: Vec<_> = self
                .pending
                .lock()
                .map(|guard| guard.keys().copied().collect())
                .unwrap_or_default();
            if batch.is_empty() {
                tokio::select! {
                    _ = &mut self.shutdown_rx => return,
                    () = self.notify.notified() => continue,
                }
            }
            #[cfg(test)]
            WorkerGates::wait(&self.gates.after_batch).await;
            let mut completed = Vec::new();
            for (endpoint_id, connection_id) in batch {
                if let DisconnectAttempt::DoneAt(generation) = self
                    .access
                    .try_disconnect_revoked(&self.clients, (endpoint_id, connection_id, None))
                {
                    completed.push(((endpoint_id, connection_id), generation));
                }
            }
            #[cfg(test)]
            WorkerGates::wait(&self.gates.before_remove).await;
            let has_remaining = self.pending.lock().is_ok_and(|mut guard| {
                for (connection, completed_generation) in completed {
                    if guard
                        .get(&connection)
                        .is_some_and(|&queued| queued <= completed_generation)
                    {
                        guard.remove(&connection);
                    }
                }
                !guard.is_empty()
            });
            if has_remaining {
                tokio::select! {
                    _ = &mut self.shutdown_rx => return,
                    () = tokio::time::sleep(PENDING_DISCONNECT_RETRY_INTERVAL) => {}
                    () = self.notify.notified() => {}
                }
            }
        }
    }
}

pub(crate) fn apply_space_authorizations<C: DisconnectClients>(
    (access, clients, pending_disconnects): (&PrivateRelayAccess, &C, &PendingDisconnects),
    authorizations: &[SpaceAuthorizationView],
) {
    let (generation, revoked) = access.replace_from_spaces_with_generation(authorizations);
    let mut deferred = Vec::new();
    for (endpoint_id, connection_id) in revoked {
        if matches!(
            access.try_disconnect_revoked(clients, (endpoint_id, connection_id, Some(generation))),
            DisconnectAttempt::Deferred
        ) {
            deferred.push((endpoint_id, connection_id));
        }
    }
    pending_disconnects.reconcile(access, (generation, deferred));
}
