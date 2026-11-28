use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh_base::EndpointId as IrohEndpointId;
use iroh_relay::server::{Access, AccessControl, ClientRequest, ConnectionId, clients::Clients};
use ma2a_core::{EndpointId, SpaceAuthorizationView, SpaceId};
use tokio::sync::{Notify, oneshot};

const PENDING_DISCONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(test)]
type OnConnectGate = (
    tokio::sync::oneshot::Sender<()>,
    tokio::sync::oneshot::Receiver<()>,
);

#[derive(Debug, Default)]
struct AccessState {
    allowed: BTreeSet<EndpointId>,
    connections: HashSet<(IrohEndpointId, ConnectionId)>,
    #[cfg(test)]
    on_connect_gate: Option<OnConnectGate>,
}

/// Current provider eligibility and admitted relay connections.
#[derive(Clone, Debug)]
pub struct PrivateRelayAccess {
    provider_endpoint_id: EndpointId,
    configured_spaces: BTreeSet<SpaceId>,
    state: Arc<Mutex<AccessState>>,
}

impl PrivateRelayAccess {
    /// Creates fail-closed admission for one provider and configured Space set.
    pub fn new(provider_endpoint_id: EndpointId, configured_spaces: &[SpaceId]) -> Self {
        Self {
            provider_endpoint_id,
            configured_spaces: configured_spaces.iter().copied().collect(),
            state: Arc::new(Mutex::new(AccessState::default())),
        }
    }

    /// Atomically replaces effective admission and returns connections losing all eligibility.
    pub fn replace_from_spaces(
        &self,
        authorizations: &[SpaceAuthorizationView],
    ) -> Vec<(IrohEndpointId, ConnectionId)> {
        let allowed = authorizations
            .iter()
            .filter(|authorization| {
                self.configured_spaces.contains(&authorization.space_id())
                    && authorization.allows_private_relay_provider(self.provider_endpoint_id)
            })
            .flat_map(SpaceAuthorizationView::member_endpoint_ids)
            .collect::<BTreeSet<_>>();
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        // Keep tracked connections until on_disconnect fires so a revocation during an in-flight
        // handshake remains visible to retries and later updates.
        let revoked = state
            .connections
            .iter()
            .copied()
            .filter(|connection| !allowed.contains(&EndpointId::from(connection.0)))
            .collect();
        state.allowed = allowed;
        revoked
    }

    /// Returns whether the authenticated Endpoint is in the current effective snapshot.
    pub fn admits(&self, endpoint_id: EndpointId) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.allowed.contains(&endpoint_id))
    }

    pub(crate) fn is_connection_revoked(
        &self,
        endpoint_id: IrohEndpointId,
        connection_id: ConnectionId,
    ) -> bool {
        self.state.lock().is_ok_and(|state| {
            state.connections.contains(&(endpoint_id, connection_id))
                && !state.allowed.contains(&EndpointId::from(endpoint_id))
        })
    }

    #[cfg(test)]
    pub(crate) fn arm_on_connect_gate(
        &self,
    ) -> (
        tokio::sync::oneshot::Receiver<()>,
        tokio::sync::oneshot::Sender<()>,
    ) {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        if let Ok(mut state) = self.state.lock() {
            state.on_connect_gate = Some((entered_tx, release_rx));
        }
        (entered_rx, release_tx)
    }
}

impl AccessControl for PrivateRelayAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        #[cfg(test)]
        let gate;
        {
            let Ok(mut state) = self.state.lock() else {
                return Access::Deny { reason: None };
            };
            if !state.allowed.contains(&request.endpoint_id().into()) {
                return Access::Deny { reason: None };
            }
            state
                .connections
                .insert((request.endpoint_id(), request.connection_id()));
            #[cfg(test)]
            {
                gate = state.on_connect_gate.take();
            }
        }
        #[cfg(test)]
        if let Some((entered, release)) = gate {
            let _entered = entered.send(());
            let _released = release.await;
        }
        Access::Allow
    }

    fn on_disconnect(&self, endpoint_id: IrohEndpointId, connection_id: ConnectionId) {
        if let Ok(mut state) = self.state.lock() {
            state.connections.remove(&(endpoint_id, connection_id));
        }
    }
}

#[derive(Debug)]
pub(crate) struct PendingDisconnects {
    pending: Arc<Mutex<HashSet<(IrohEndpointId, ConnectionId)>>>,
    notify: Arc<Notify>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PendingDisconnects {
    pub(crate) fn spawn(access: PrivateRelayAccess, clients: Clients) -> Self {
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let notify = Arc::new(Notify::new());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(
            PendingDisconnectWorker {
                access,
                clients,
                pending: Arc::clone(&pending),
                notify: Arc::clone(&notify),
                shutdown_rx,
            }
            .run(),
        );
        Self {
            pending,
            notify,
            shutdown_tx: Some(shutdown_tx),
            task: Some(task),
        }
    }

    pub(crate) fn enqueue(
        &self,
        connections: impl IntoIterator<Item = (IrohEndpointId, ConnectionId)>,
    ) {
        let mut added = false;
        if let Ok(mut pending) = self.pending.lock() {
            for connection in connections {
                added |= pending.insert(connection);
            }
        }
        if added {
            self.notify.notify_one();
        }
    }

    pub(crate) fn retain_revoked(&self, access: &PrivateRelayAccess) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.retain(|&(endpoint_id, connection_id)| {
                access.is_connection_revoked(endpoint_id, connection_id)
            });
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

struct PendingDisconnectWorker {
    access: PrivateRelayAccess,
    clients: Clients,
    pending: Arc<Mutex<HashSet<(IrohEndpointId, ConnectionId)>>>,
    notify: Arc<Notify>,
    shutdown_rx: oneshot::Receiver<()>,
}

impl PendingDisconnectWorker {
    async fn run(mut self) {
        loop {
            let batch: Vec<_> = self
                .pending
                .lock()
                .map(|guard| guard.iter().copied().collect())
                .unwrap_or_default();
            if batch.is_empty() {
                tokio::select! {
                    _ = &mut self.shutdown_rx => return,
                    () = self.notify.notified() => continue,
                }
            }
            let mut completed = Vec::new();
            for (endpoint_id, connection_id) in batch {
                if !self
                    .access
                    .is_connection_revoked(endpoint_id, connection_id)
                    || self.clients.disconnect(endpoint_id, Some(connection_id))
                {
                    completed.push((endpoint_id, connection_id));
                }
            }
            let has_remaining = self.pending.lock().is_ok_and(|mut guard| {
                for connection in completed {
                    guard.remove(&connection);
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
