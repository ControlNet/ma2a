use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex},
};

use iroh_base::EndpointId as IrohEndpointId;
use iroh_relay::server::{Access, AccessControl, ClientRequest, ConnectionId, clients::Clients};
use ma2a_core::{EndpointId, SpaceAuthorizationView, SpaceId};
#[cfg(test)]
use tokio::sync::oneshot;

mod pending;
pub(crate) use pending::{PendingDisconnects, apply_space_authorizations};

#[cfg(test)]
type OnConnectGate = (oneshot::Sender<()>, oneshot::Receiver<()>);

pub(crate) trait DisconnectClients: Send + Sync + 'static {
    fn disconnect(&self, endpoint_id: IrohEndpointId, connection_id: Option<ConnectionId>) -> bool;
}

impl DisconnectClients for Clients {
    fn disconnect(&self, endpoint_id: IrohEndpointId, connection_id: Option<ConnectionId>) -> bool {
        Self::disconnect(self, endpoint_id, connection_id)
    }
}

#[derive(Debug, Default)]
struct AccessState {
    generation: u64,
    allowed: BTreeSet<EndpointId>,
    connections: HashSet<(IrohEndpointId, ConnectionId)>,
    #[cfg(test)]
    on_connect_gate: Option<OnConnectGate>,
}

enum DisconnectAttempt {
    DoneAt(u64),
    Deferred,
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
        self.replace_from_spaces_with_generation(authorizations).1
    }

    fn replace_from_spaces_with_generation(
        &self,
        authorizations: &[SpaceAuthorizationView],
    ) -> (u64, Vec<(IrohEndpointId, ConnectionId)>) {
        let allowed = authorizations
            .iter()
            .filter(|authorization| {
                self.configured_spaces.contains(&authorization.space_id())
                    && authorization.allows_private_relay_provider(self.provider_endpoint_id)
            })
            .flat_map(SpaceAuthorizationView::member_endpoint_ids)
            .collect::<BTreeSet<_>>();
        let Ok(mut state) = self.state.lock() else {
            return (0, Vec::new());
        };
        state.generation = state.generation.wrapping_add(1);
        // Keep tracked connections until on_disconnect fires so a revocation during an in-flight
        // handshake remains visible to retries and later updates.
        let revoked = state
            .connections
            .iter()
            .copied()
            .filter(|connection| !allowed.contains(&EndpointId::from(connection.0)))
            .collect();
        state.allowed = allowed;
        (state.generation, revoked)
    }

    /// Returns whether the authenticated Endpoint is in the current effective snapshot.
    pub fn admits(&self, endpoint_id: EndpointId) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.allowed.contains(&endpoint_id))
    }

    fn try_disconnect_revoked<C: DisconnectClients>(
        &self,
        clients: &C,
        (endpoint_id, connection_id, expected_generation): (
            IrohEndpointId,
            ConnectionId,
            Option<u64>,
        ),
    ) -> DisconnectAttempt {
        let Ok(state) = self.state.lock() else {
            return DisconnectAttempt::Deferred;
        };
        if expected_generation.is_some_and(|generation| state.generation != generation)
            || !state.connections.contains(&(endpoint_id, connection_id))
            || state.allowed.contains(&EndpointId::from(endpoint_id))
        {
            return DisconnectAttempt::DoneAt(state.generation);
        }
        if clients.disconnect(endpoint_id, Some(connection_id)) {
            DisconnectAttempt::DoneAt(state.generation)
        } else {
            DisconnectAttempt::Deferred
        }
    }

    #[cfg(test)]
    pub(crate) fn arm_on_connect_gate(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (entered_tx, entered_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
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

#[cfg(test)]
#[path = "relay_access_test.rs"]
mod tests;
