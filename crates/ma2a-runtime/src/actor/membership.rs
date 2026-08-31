use std::collections::BTreeSet;

use ma2a_core::{MemberCapabilities, SpaceId, SpaceMemberV1, SpacePolicyV1};

use super::Actor;
use crate::{error::RuntimeError, state::RuntimeEvent};

impl Actor {
    pub(super) async fn create_owned_space(&mut self) -> Result<SpaceId, RuntimeError> {
        let created_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))?;
        let member = SpaceMemberV1::new(
            self.state.endpoint_id,
            "local-endpoint".to_owned(),
            MemberCapabilities::new(true, true),
        )
        .map_err(|_| crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Control))?;
        let created = self
            .store
            .create_owned_space(ma2a_store::SpaceCreation::new(
                created_at_ms,
                member,
                SpacePolicyV1::phase_one_default(),
            ))
            .await?;
        let space_id = created.space_id();
        self.state.revision = created.revision();
        self.state.memberships.insert(space_id);
        self.synchronized_control_peers.clear();
        self.endpoint.set_control_enabled(true);
        self.refresh_control_lookup().await?;
        self.refresh_relay_candidates().await?;
        self.refresh_private_relay_access().await?;
        self.schedule_control_round(
            crate::control_sync::ControlRoundTrigger::ManifestAdvanced,
            None,
        );
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(created.revision()));
        Ok(space_id)
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
        self.refresh_control_lookup().await?;
        self.schedule_control_round(
            crate::control_sync::ControlRoundTrigger::ManifestAdvanced,
            None,
        );
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        Ok(revision)
    }

    pub(super) async fn revoke_owned_space_member(
        &mut self,
        space_id: SpaceId,
        endpoint_id: ma2a_core::EndpointId,
    ) -> Result<u64, RuntimeError> {
        let issued_at_ms = u64::try_from(self.clock.now_ms()?)
            .map_err(|_| crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Clock))?;
        let (revision, memberships) = self
            .store
            .revoke_owned_space_member(crate::store::OwnedMemberRevocation {
                space_id,
                endpoint_id,
                issued_at_ms,
                local_endpoint_id: self.state.endpoint_id,
            })
            .await?;
        self.state.revision = revision;
        self.state.memberships = memberships;
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        self.refresh_control_lookup().await?;
        self.refresh_relay_candidates().await?;
        self.refresh_private_relay_access().await?;
        self.schedule_control_round(
            crate::control_sync::ControlRoundTrigger::ManifestAdvanced,
            None,
        );
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        Ok(revision)
    }
}
