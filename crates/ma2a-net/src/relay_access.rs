use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, Mutex},
};

use iroh_base::EndpointId as IrohEndpointId;
use iroh_relay::server::{Access, AccessControl, ClientRequest, ConnectionId};
use ma2a_core::{EndpointId, SpaceAuthorizationView, SpaceId};

#[derive(Debug, Default)]
struct AccessState {
    allowed: BTreeSet<EndpointId>,
    connections: HashSet<(IrohEndpointId, ConnectionId)>,
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
        let mut revoked = Vec::new();
        state.connections.retain(|connection| {
            if allowed.contains(&EndpointId::from(connection.0)) {
                true
            } else {
                revoked.push(*connection);
                false
            }
        });
        state.allowed = allowed;
        revoked
    }

    /// Returns whether the authenticated Endpoint is in the current effective snapshot.
    pub fn admits(&self, endpoint_id: EndpointId) -> bool {
        self.state
            .lock()
            .is_ok_and(|state| state.allowed.contains(&endpoint_id))
    }
}

impl AccessControl for PrivateRelayAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let Ok(mut state) = self.state.lock() else {
            return Access::Deny { reason: None };
        };
        if !state.allowed.contains(&request.endpoint_id().into()) {
            return Access::Deny { reason: None };
        }
        state
            .connections
            .insert((request.endpoint_id(), request.connection_id()));
        Access::Allow
    }

    fn on_disconnect(&self, endpoint_id: IrohEndpointId, connection_id: ConnectionId) {
        if let Ok(mut state) = self.state.lock() {
            state.connections.remove(&(endpoint_id, connection_id));
        }
    }
}
