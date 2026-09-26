use crate::{
    RuntimeError,
    actor::Actor,
    control_sync::{ControlChanges, ControlFailure},
};
use ma2a_core::EndpointId;
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct PendingControl {
    changes: ControlChanges,
    peers: BTreeSet<EndpointId>,
}

impl Actor {
    pub(crate) fn retain_control_completion(
        &mut self,
        changes: ControlChanges,
        peers: BTreeSet<EndpointId>,
    ) {
        let pending = self
            .maintenance
            .control_completion
            .get_or_insert_with(PendingControl::default);
        pending.changes.merge(changes);
        pending.peers.extend(peers);
    }

    pub(crate) async fn reconcile_control_completion(&mut self) -> Result<(), RuntimeError> {
        let Some(pending) = &self.maintenance.control_completion else {
            return Ok(());
        };
        let peers = pending.peers.clone();
        // A completed task may be older than a local departure. Read one current
        // Store receipt, never install the task's stale lookup/membership snapshot.
        let projection = self.store.memberships(self.state.endpoint_id).await?;
        let changed = self.state.memberships != *projection.value();
        self.state.revision = self.state.revision.max(projection.revision());
        self.state.memberships = projection.into_value();
        if changed {
            self.synchronized_control_peers.clear();
        }
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        self.refresh_control_lookup().await?;
        self.refresh_relay_candidates().await?;
        self.refresh_private_relay_access().await?;
        self.refresh_local_control_publications().await?;
        let mut eligible = BTreeSet::new();
        for peer in peers {
            match self
                .store
                .authorize_control(self.state.endpoint_id, peer)
                .await
            {
                Ok(()) => {
                    eligible.insert(peer);
                }
                Err(ControlFailure::Rejected(_)) => {}
                Err(ControlFailure::Runtime(error)) => return Err(error),
            }
        }
        self.synchronized_control_peers.extend(eligible);
        if let Some(pending) = self.maintenance.control_completion.take() {
            self.schedule_control_changes(pending.changes);
        }
        Ok(())
    }

    pub(crate) fn control_failure(error: ControlFailure) -> Result<(), RuntimeError> {
        match error {
            ControlFailure::Rejected(_) => Ok(()),
            ControlFailure::Runtime(error) => {
                Self::absorb_background("control completion", Err(error))
            }
        }
    }
}
