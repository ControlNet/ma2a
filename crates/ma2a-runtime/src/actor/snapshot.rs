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
                SpaceView::new(
                    space.space_id(),
                    &crate::api::encode_hex(space.space_id().as_bytes()),
                    space.member_count(),
                )
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let endpoint = EndpointView::new(self.state.endpoint_id, env!("CARGO_PKG_VERSION"), true)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let collections = SnapshotCollections::new(
            spaces,
            ControlSyncView::new(Vec::new(), false)
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?,
            Vec::new(),
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let state = SnapshotState::new(
            NetworkSnapshotState::new(
                ObservedRelayStateView::new(false, false),
                ReachabilityView::new(false, false),
            ),
            ClientSnapshotState::new(
                EchoSummaryView::new(0, 0),
                UiAuthView::new(true, durable.password_set(), 0),
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
