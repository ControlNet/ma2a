use std::collections::BTreeSet;

use ma2a_core::{MemberCapabilities, SpaceId, SpaceMemberV1, SpacePolicyV1, default_member_label};

use super::Actor;
use crate::{error::RuntimeError, state::RuntimeEvent};

impl Actor {
    pub(super) async fn create_owned_space(
        &mut self,
        name: String,
    ) -> Result<ma2a_store::CreatedSpace, RuntimeError> {
        let created_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))?;
        // The Space name names the Space; the creator's member label names the
        // creating Endpoint. Reusing one as the other makes a Space read as a member.
        let member = SpaceMemberV1::new(
            self.state.endpoint_id,
            default_member_label(self.state.endpoint_id),
            MemberCapabilities::new(true, true),
        )
        .map_err(|_| crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Control))?;
        let created = self
            .store
            .create_owned_space(
                ma2a_store::SpaceCreation::new(
                    created_at_ms,
                    member,
                    SpacePolicyV1::phase_one_default(),
                )
                .with_name(name),
            )
            .await?;
        let space_id = created.space_id();
        self.state.revision = created.revision();
        self.state.memberships.insert(space_id);
        self.synchronized_control_peers.clear();
        self.endpoint.set_control_enabled(true);
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(created.revision()));
        self.refresh_after_membership_change()
            .await
            .map_err(|error| error.after_commit(created.revision()))?;
        Ok(created)
    }

    pub(super) async fn observe_memberships(
        &mut self,
        memberships: Vec<SpaceId>,
    ) -> Result<u64, RuntimeError> {
        let mut candidate = self.state.clone();
        candidate.memberships = memberships.into_iter().collect::<BTreeSet<_>>();
        let revision = self.store.observe(&candidate).await?;
        candidate.revision = revision;
        self.state = candidate;
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        self.refresh_after_membership_change()
            .await
            .map_err(|error| error.after_commit(revision))?;
        Ok(revision)
    }

    pub(super) async fn revoke_owned_space_member(
        &mut self,
        space_id: SpaceId,
        endpoint_id: ma2a_core::EndpointId,
    ) -> Result<crate::store::RemovedMember, RuntimeError> {
        let issued_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))?;
        let removed = self
            .store
            .revoke_owned_space_member(crate::store::OwnedMemberRevocation {
                space_id,
                endpoint_id,
                issued_at_ms,
                local_endpoint_id: self.state.endpoint_id,
            })
            .await
            .map_err(|error| self.retain_membership_error(error))?;
        let revision = removed.revision;
        self.state.revision = removed.projection_revision;
        self.state.memberships.clone_from(&removed.memberships);
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        self.refresh_after_membership_change()
            .await
            .map_err(|error| error.after_commit(revision))?;
        Ok(removed)
    }

    pub(crate) fn retain_membership_error(&mut self, error: RuntimeError) -> RuntimeError {
        if let Some(revision) = error.committed_revision() {
            self.state.revision = self.state.revision.max(revision);
            self.retain_control_completion(
                crate::control_sync::ControlChanges {
                    manifest: true,
                    address: false,
                    relay: false,
                },
                BTreeSet::new(),
            );
            let _receivers = self
                .events
                .send(RuntimeEvent::memberships_changed(revision));
        }
        error
    }

    pub(super) async fn advance_owned_space(
        &mut self,
        update: ma2a_store::OwnedSpaceUpdate,
    ) -> Result<u64, RuntimeError> {
        let (revision, memberships) = self
            .store
            .advance_owned_space(update, self.state.endpoint_id)
            .await
            .map_err(|error| self.retain_membership_error(error))?;
        self.state.revision = revision;
        self.state.memberships = memberships;
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        let _receivers = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        self.refresh_after_membership_change()
            .await
            .map_err(|error| error.after_commit(revision))?;
        Ok(revision)
    }

    /// Re-derives every projection that depends on the signed membership set.
    pub(crate) async fn refresh_after_membership_change(&mut self) -> Result<(), RuntimeError> {
        self.maintenance
            .membership_pending
            .get_or_insert(crate::control_sync::ControlRoundTrigger::ManifestAdvanced);
        self.refresh_control_lookup().await?;
        self.refresh_relay_candidates().await?;
        self.refresh_private_relay_access().await?;
        self.refresh_local_control_publications().await?;
        if let Some(trigger) = self.maintenance.membership_pending.take() {
            self.schedule_control_round(trigger, None);
        }
        Ok(())
    }

    pub(crate) async fn reconcile_membership_completion(&mut self) -> Result<(), RuntimeError> {
        self.reconcile_control_completion().await?;
        if self.maintenance.membership_pending.is_some() {
            self.refresh_after_membership_change().await?;
        }
        Ok(())
    }
}
