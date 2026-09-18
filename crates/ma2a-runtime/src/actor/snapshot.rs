use ma2a_core::EchoResultClass;
use tokio::sync::oneshot;

use super::Actor;
use crate::{
    api::{
        ClientSnapshotState, ControlSyncView, EchoSummaryView, EndpointView, NetworkSnapshotState,
        ObservedRelayStateView, ReachabilityView, RuntimeSnapshot, SnapshotCollections,
        SnapshotHeader, SnapshotState, UiAuthView,
    },
    error::{RuntimeError, RuntimeErrorKind},
};

mod project;

use project::{connection_view, space_view};

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
            .map(space_view)
            .collect::<Result<Vec<_>, _>>()?;
        let endpoint = EndpointView::new(self.state.endpoint_id, env!("CARGO_PKG_VERSION"), true)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let collections = SnapshotCollections::new(
            spaces,
            ControlSyncView::new(self.synchronized_control_peers.iter().copied().collect())
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?,
            self.connections
                .latest_observations()
                .iter()
                .map(|latest| connection_view(latest, &self.connections))
                .collect::<Result<Vec<_>, _>>()?,
            self.state
                .relay
                .private_coverage()
                .map(|(candidate, covered)| {
                    crate::api::RelayCandidateView::new(
                        candidate.provider_endpoint_id(),
                        "private",
                        self.state.relay.home_relay_compatible(candidate),
                        covered.iter().copied().collect(),
                    )
                    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
                })
                .collect::<Result<Vec<_>, _>>()?,
            self.control_round_history.iter().copied().collect(),
        )
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let (echo_successes, echo_failures) = self.echo_audit.snapshot().iter().fold(
            (0_u32, 0_u32),
            |(successes, failures), record| match record.result_class() {
                EchoResultClass::Succeeded => (successes.saturating_add(1), failures),
                EchoResultClass::Unauthorized
                | EchoResultClass::InvalidInput
                | EchoResultClass::TimedOut
                | EchoResultClass::ConcurrencyExceeded
                | EchoResultClass::Cancelled
                | EchoResultClass::Unavailable => (successes, failures.saturating_add(1)),
            },
        );
        let state = SnapshotState::new(
            NetworkSnapshotState::new(
                ObservedRelayStateView::new(
                    self.private_relay_server.is_some(),
                    self.state.relay.public_relay_online(),
                ),
                ReachabilityView::new(
                    self.state.direct_reachable,
                    matches!(
                        self.state.relay_reachability(),
                        ma2a_core::RelayReachability::IrohHomeConnected
                    ),
                ),
            ),
            ClientSnapshotState::new(
                EchoSummaryView::new(echo_successes, echo_failures),
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

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use ma2a_core::RequestId;
    use ma2a_net::EndpointSecret;
    use ma2a_store::StoreConfig;

    use crate::Runtime;

    type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
    static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

    struct TempState(PathBuf);

    impl TempState {
        fn new(name: &str) -> TestResult<Self> {
            let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ma2a-snapshot-{name}-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            }
            Ok(Self(path))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempState {
        fn drop(&mut self) {
            let _cleanup = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn snapshot_echo_summary_uses_runtime_owned_outcomes() -> TestResult {
        // Given
        let state = TempState::new("echo-summary")?;
        let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
        let target = EndpointSecret::generate().endpoint_id();
        let request_id = RequestId::try_from([0x73; 16].as_slice())?;

        // When
        let result = runtime.handle().echo(request_id, target, b"probe").await;
        let snapshot = runtime.handle().snapshot().await?.to_value();
        runtime.shutdown().await?;

        // Then
        assert!(result.is_err());
        assert_eq!(
            snapshot.pointer("/recent_echo_summary/failures"),
            Some(&serde_json::json!(1))
        );
        assert_eq!(
            snapshot.pointer("/connections/0/state"),
            Some(&serde_json::json!("failed"))
        );
        assert_eq!(
            snapshot.pointer("/connections/0/path"),
            Some(&serde_json::json!("mixed_or_unknown"))
        );
        assert_eq!(
            snapshot.pointer("/connections/0/rtt_ms"),
            Some(&serde_json::Value::Null)
        );
        assert!(snapshot.pointer("/control_sync/synchronized").is_none());
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn echo_outcome_advances_the_authoritative_snapshot_revision() -> TestResult {
        // Given
        let state = TempState::new("echo-revision")?;
        let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
        let before = runtime.handle().snapshot().await?.to_value();
        let target = EndpointSecret::generate().endpoint_id();
        let request_id = RequestId::try_from([0x74; 16].as_slice())?;

        // When
        let result = runtime.handle().echo(request_id, target, b"probe").await;
        let after = runtime.handle().snapshot().await?.to_value();
        runtime.shutdown().await?;

        // Then
        assert!(result.is_err());
        assert!(
            after
                .pointer("/revision")
                .and_then(serde_json::Value::as_u64)
                > before
                    .pointer("/revision")
                    .and_then(serde_json::Value::as_u64)
        );
        assert_eq!(
            after.pointer("/recent_echo_summary/failures"),
            Some(&serde_json::json!(1))
        );
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn equal_revision_snapshots_have_equal_echo_summaries() -> TestResult {
        // Given
        let state = TempState::new("echo-same-revision")?;
        let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
        let before = runtime.handle().snapshot().await?.to_value();
        let target = EndpointSecret::generate().endpoint_id();
        let request_id = RequestId::try_from([0x75; 16].as_slice())?;

        // When
        let _result = runtime.handle().echo(request_id, target, b"probe").await;
        let after = runtime.handle().snapshot().await?.to_value();
        runtime.shutdown().await?;

        // Then
        if before.pointer("/revision") == after.pointer("/revision") {
            assert_eq!(
                before.pointer("/recent_echo_summary"),
                after.pointer("/recent_echo_summary")
            );
        }
        Ok(())
    }
}
