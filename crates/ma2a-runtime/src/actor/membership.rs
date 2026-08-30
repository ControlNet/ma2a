use std::collections::BTreeSet;

use ma2a_core::SpaceId;

use super::Actor;
use crate::{error::RuntimeError, state::RuntimeEvent};

impl Actor {
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
}
