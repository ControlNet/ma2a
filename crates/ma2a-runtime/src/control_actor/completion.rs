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
    generation: u64,
    announced: bool,
}

impl Actor {
    pub(crate) fn clear_synchronized_control_peers(&mut self) {
        let had_peers = !self.synchronized_control_peers.peers().is_empty();
        self.synchronized_control_peers.clear();
        if had_peers {
            self.view_sequence = self.view_sequence.saturating_add(1);
        }
    }

    pub(crate) fn retain_control_completion(
        &mut self,
        changes: ControlChanges,
        peers: BTreeSet<EndpointId>,
    ) {
        if changes.manifest && peers.is_empty() {
            // A manifest error committed outside a round invalidates in-flight rounds immediately.
            self.clear_synchronized_control_peers();
        }
        let generation = self.synchronized_control_peers.generation();
        let pending = self
            .maintenance
            .control_completion
            .get_or_insert_with(|| PendingControl {
                generation,
                ..PendingControl::default()
            });
        pending.changes.merge(changes);
        if pending.generation != generation {
            pending.peers.clear();
            pending.generation = generation;
        }
        if changes.manifest {
            pending.announced = false;
            pending.peers.clear();
        }
        pending.peers.extend(peers);
    }

    pub(crate) async fn reconcile_control_completion(&mut self) -> Result<(), RuntimeError> {
        let before_peers = self.synchronized_control_peers.peers().clone();
        let result = self.reconcile_control_completion_inner().await;
        if self.synchronized_control_peers.peers() != &before_peers {
            self.view_sequence = self.view_sequence.saturating_add(1);
        }
        result
    }

    async fn reconcile_control_completion_inner(&mut self) -> Result<(), RuntimeError> {
        let Some(pending) = &self.maintenance.control_completion else {
            return Ok(());
        };
        let peers = if pending.generation == self.synchronized_control_peers.generation() {
            pending.peers.clone()
        } else {
            BTreeSet::new()
        };
        let manifest_advanced = pending.changes.manifest;
        let already_announced = pending.announced;
        // A completed task may be older than a local departure. Read one current
        // Store receipt, never install the task's stale lookup/membership snapshot.
        let projection = self.store.memberships(self.state.endpoint_id).await?;
        let changed = self.state.memberships != *projection.value();
        self.state.revision = self.state.revision.max(projection.revision());
        self.state.memberships = projection.into_value();
        if changed || manifest_advanced {
            self.synchronized_control_peers.clear();
            if let Some(pending) = self.maintenance.control_completion.as_mut() {
                pending.generation = self.synchronized_control_peers.generation();
                if !manifest_advanced {
                    pending.peers.clear();
                }
            }
        }
        let peers = if changed && !manifest_advanced {
            BTreeSet::new()
        } else {
            peers
        };
        if changed || (manifest_advanced && !already_announced) {
            if let Some(pending) = self.maintenance.control_completion.as_mut() {
                pending.announced = true;
            }
            let _receivers = self
                .events
                .send(crate::state::RuntimeEvent::memberships_changed(
                    self.state.revision,
                ));
        }
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        self.refresh_control_lookup().await?;
        self.refresh_relay_candidates().await?;
        self.refresh_private_relay_access().await?;
        if let Err(error) = self.refresh_local_control_publications().await {
            if error.is_deferred_publication() {
                Self::absorb_background("control publication", Err(error))?;
            } else {
                return Err(error);
            }
        }
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
