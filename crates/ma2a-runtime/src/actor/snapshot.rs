use tokio::sync::oneshot;

use super::Actor;
use crate::{
    api::{
        ClientSnapshotState, ControlSyncView, EchoSummaryView, EndpointView, NetworkSnapshotState,
        ObservedRelayStateView, ReachabilityView, RuntimeSnapshot, SnapshotCollections,
        SnapshotHeader, SnapshotState, SpaceView, UiAuthView,
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
                SpaceView::new(space.space_id(), space.label(), space.member_count())
                    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let endpoint = EndpointView::new(self.state.endpoint_id, env!("CARGO_PKG_VERSION"), true)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let collections = SnapshotCollections::new(
            spaces,
            ControlSyncView::new(
                self.synchronized_control_peers.iter().copied().collect(),
                !self.synchronized_control_peers.is_empty(),
            )
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?,
            self.state
                .relay
                .private_candidates()
                .map(|candidate| {
                    crate::api::RelayCandidateView::new(
                        candidate.provider_endpoint_id(),
                        "private",
                        true,
                    )
                    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let state = SnapshotState::new(
            NetworkSnapshotState::new(
                ObservedRelayStateView::new(
                    self.private_relay_server.is_some(),
                    self.state.relay.public_relay_online(),
                ),
                ReachabilityView::new(
                    self.state.connectivity.is_direct_only(),
                    matches!(
                        self.state.relay_reachability(),
                        ma2a_core::RelayReachability::IrohHomeConnected
                    ),
                ),
            ),
            ClientSnapshotState::new(
                EchoSummaryView::new(0, 0),
                UiAuthView::new(true, durable.password_set(), durable.active_sessions()),
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
