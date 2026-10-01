//! Owner-signed service grant changes, applied through the single-owner actor.

use ma2a_core::{EndpointId, SpaceId};

use super::Actor;
use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    state::RuntimeEvent,
};

impl Actor {
    /// Signs the next manifest with one member's service grants replaced.
    ///
    /// A refusal signs and commits nothing. An identical grant succeeds without a new
    /// generation, so replaying the intent never advances the chain twice.
    pub(super) async fn grant_owned_space_member(
        &mut self,
        (space_id, endpoint_id): (SpaceId, EndpointId),
        services: [bool; 8],
    ) -> crate::store::GrantResult {
        let issued_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let granted = match self
            .store
            .grant_owned_space_member(crate::store::OwnedMemberGrant {
                space_id,
                endpoint_id,
                services,
                issued_at_ms,
                local_endpoint_id: self.state.endpoint_id,
            })
            .await
            .map_err(|error| self.retain_membership_error(error))?
        {
            Ok(granted) => granted,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if !granted.advanced {
            return Ok(Ok(granted));
        }
        let revision = granted.revision;
        self.state.revision = granted.projection_revision;
        self.state.memberships.clone_from(&granted.memberships);
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        let _receivers = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        self.refresh_after_membership_change()
            .await
            .map_err(|error| error.after_commit(revision))?;
        Ok(Ok(granted))
    }
}
