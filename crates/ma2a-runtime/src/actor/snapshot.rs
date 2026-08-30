use ma2a_core::{EchoResultClass, RelayReachability};
use tokio::sync::oneshot;

use super::Actor;
use crate::{
    api::{
        ClientSnapshotState, ControlSyncView, EchoSummaryView, EndpointView, NetworkSnapshotState,
        ObservedRelayStateView, ReachabilityView, RelayCandidateView, RuntimeSnapshot,
        SnapshotCollections, SnapshotHeader, SnapshotState, SpaceView, UiAuthView,
    },
    error::{RuntimeError, RuntimeErrorKind},
};

impl Actor {
    pub(super) async fn handle_snapshot(
        &self,
        reply: oneshot::Sender<Result<RuntimeSnapshot, RuntimeError>>,
    ) {
        let _unsent = reply.send(self.snapshot().await);
    }

    pub(super) async fn snapshot(&self) -> Result<RuntimeSnapshot, RuntimeError> {
        let durable = self
            .store
            .snapshot(self.state.endpoint_id, self.clock.now_ms()?)
            .await?;
        let spaces = durable
            .spaces()
            .iter()
            .map(|space| {
                SpaceView::new(
                    space.space_id(),
                    &crate::api::encode_hex(space.space_id().as_bytes()),
                    space.member_count(),
                )
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let control_peers = self
            .synchronized_control_peers
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let audit = self.echo_audit.snapshot();
        let successes = u32::try_from(
            audit
                .iter()
                .filter(|record| record.result_class() == EchoResultClass::Succeeded)
                .count(),
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let failures = u32::try_from(audit.len())
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?
            .saturating_sub(successes);
        let relay_connected = matches!(
            self.state.relay_reachability(),
            RelayReachability::IrohHomeConnected
        );
        let (private_relay_online, public_relay_online) = self.state.relay.observed_relay_state();
        let relay_candidates = self
            .state
            .relay
            .private_candidates()
            .map(|(candidate, eligible)| {
                RelayCandidateView::new(candidate.provider_endpoint_id(), "private", eligible)
                    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let direct = self.state.endpoint_addr.ip_addrs().next().is_some();
        let endpoint = EndpointView::new(
            self.state.endpoint_id,
            env!("CARGO_PKG_VERSION"),
            self.state.ready,
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let collections = SnapshotCollections::new(
            spaces,
            ControlSyncView::new(
                control_peers,
                self.synchronized_control_peers.len() == self.state.memberships.len(),
            )
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?,
            relay_candidates,
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let state = SnapshotState::new(
            NetworkSnapshotState::new(
                ObservedRelayStateView::new(private_relay_online, public_relay_online),
                ReachabilityView::new(direct, relay_connected),
            ),
            ClientSnapshotState::new(
                EchoSummaryView::new(successes, failures),
                UiAuthView::new(
                    self.state.ready,
                    durable.password_set(),
                    durable.active_sessions(),
                ),
            ),
        );
        RuntimeSnapshot::new(
            SnapshotHeader::new(durable.revision(), endpoint),
            collections,
            state,
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
    }
}
