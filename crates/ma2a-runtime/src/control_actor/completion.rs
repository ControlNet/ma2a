use crate::{
    RuntimeError,
    actor::Actor,
    control_sync::{ControlChanges, ControlFailure},
};
use ma2a_core::EndpointId;
use std::collections::BTreeSet;

#[derive(Default)]
struct LookupSignal(tokio::sync::watch::Sender<bool>);

impl LookupSignal {
    fn set_installed(&self, installed: bool) {
        self.0.send_if_modified(|current| {
            if *current == installed {
                false
            } else {
                *current = installed;
                true
            }
        });
    }
}

impl Drop for LookupSignal {
    fn drop(&mut self) {
        self.set_installed(true);
    }
}

pub(crate) struct PendingControl {
    changes: ControlChanges,
    peers: BTreeSet<EndpointId>,
    generation: u64,
    announced: bool,
    cleared: bool,
    lookup_signal: LookupSignal,
}

impl Default for PendingControl {
    fn default() -> Self {
        Self {
            changes: ControlChanges::default(),
            peers: BTreeSet::new(),
            generation: 0,
            announced: false,
            cleared: false,
            lookup_signal: LookupSignal(tokio::sync::watch::channel(true).0),
        }
    }
}

impl PendingControl {
    pub(crate) fn lookup_waiter(&self) -> Option<tokio::sync::watch::Receiver<bool>> {
        if *self.lookup_signal.0.borrow() {
            None
        } else {
            Some(self.lookup_signal.0.subscribe())
        }
    }
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
        let cleared_now = changes.manifest && peers.is_empty();
        if cleared_now {
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
            pending.cleared = false;
        }
        if changes.manifest {
            pending.announced = false;
            pending.peers.clear();
            pending.cleared = cleared_now;
        }
        if changes.manifest || changes.address {
            pending.lookup_signal.set_installed(false);
        }
        pending.peers.extend(peers);
    }

    pub(crate) async fn reconcile_control_completion(&mut self) -> Result<(), RuntimeError> {
        let before_peers = self.synchronized_control_peers.peers().clone();
        let before_relay = self.state.relay.clone();
        let result = self.reconcile_control_completion_inner().await;
        if self.synchronized_control_peers.peers() != &before_peers
            || self.state.relay != before_relay
        {
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
        let already_cleared =
            pending.cleared && pending.generation == self.synchronized_control_peers.generation();
        // A completed task may be older than a local departure. Read one current
        // Store receipt, never install the task's stale lookup/membership snapshot.
        let projection = self.store.memberships(self.state.endpoint_id).await?;
        let changed = self.state.memberships != *projection.value();
        self.state.revision = self.state.revision.max(projection.revision());
        self.state.memberships = projection.into_value();
        if (changed || manifest_advanced) && !already_cleared {
            self.synchronized_control_peers.clear();
            if let Some(pending) = self.maintenance.control_completion.as_mut() {
                pending.generation = self.synchronized_control_peers.generation();
                pending.cleared = true;
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
        if let Some(pending) = self.maintenance.control_completion.as_ref() {
            pending.lookup_signal.set_installed(true);
        }
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
