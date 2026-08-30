use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use ma2a_core::SpaceId;
use ma2a_net::EndpointSecret;
use ma2a_store::StoreConfig;
use tokio::sync::oneshot;

use super::ControlRoundQueue;
use crate::{
    Runtime,
    control_sync::{ControlChanges, ControlRoundRequest, ControlRoundScope, ControlRoundTrigger},
};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-scheduling-{label}-{}-{serial}",
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
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[test]
fn targeted_scope_retains_the_requested_eligible_peer() -> TestResult {
    // Given
    let local = EndpointSecret::parse(&[0x10; 32])?.endpoint_id();
    let requested = EndpointSecret::parse(&[0x20; 32])?.endpoint_id();
    let other = EndpointSecret::parse(&[0x30; 32])?.endpoint_id();

    // When
    let selected = ControlRoundRequest {
        local_endpoint_id: local,
        rotation: 7,
        now_ms: 0,
        scope: ControlRoundScope::peer(requested),
    }
    .select(&[other, requested]);

    // Then
    assert_eq!(selected, vec![requested]);
    Ok(())
}

#[test]
fn older_round_does_not_complete_a_newer_targeted_waiter() -> TestResult {
    // Given
    let peer = EndpointSecret::parse(&[0x40; 32])?.endpoint_id();
    let mut queue = ControlRoundQueue::default();
    let (older_reply, mut older_response) = oneshot::channel();
    let older = queue
        .request(ControlRoundScope::all(), Some(older_reply))
        .ok_or("older round missing")?;
    let (newer_reply, mut newer_response) = oneshot::channel();
    let pending = queue.request(ControlRoundScope::peer(peer), Some(newer_reply));

    // When
    let completed = queue.complete(older.id(), true);

    // Then
    assert!(pending.is_none());
    assert_eq!(completed.len(), 1);
    for waiter in completed {
        waiter.send(Ok(1));
    }
    assert_eq!(older_response.try_recv()??, 1);
    assert!(matches!(
        newer_response.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let newer = queue.take_pending().ok_or("pending round missing")?;
    assert_eq!(newer.scope(), &ControlRoundScope::peer(peer));
    assert_eq!(queue.complete(newer.id(), true).len(), 1);
    Ok(())
}

#[test]
fn control_queue_health_tracks_pending_and_failed_work() -> TestResult {
    // Given
    let mut queue = ControlRoundQueue::default();
    assert!(queue.is_synchronized());
    let round = queue
        .request(ControlRoundScope::all(), None)
        .ok_or("control round missing")?;

    // When
    let _completed = queue.complete(round.id(), false);

    // Then
    assert!(!queue.is_synchronized());
    Ok(())
}

#[test]
fn global_pending_work_preserves_the_targeted_peer_for_its_waiter() -> TestResult {
    // Given
    let local = EndpointSecret::parse(&[0x60; 32])?.endpoint_id();
    let mut eligible = (0x61_u8..=0x65)
        .map(|seed| EndpointSecret::parse(&[seed; 32]).map(|secret| secret.endpoint_id()))
        .collect::<Result<Vec<_>, _>>()?;
    eligible.sort_unstable();
    let requested = eligible.first().copied().ok_or("eligible peer missing")?;
    let mut queue = ControlRoundQueue::default();
    let active = queue
        .request(ControlRoundScope::all(), None)
        .ok_or("active round missing")?;
    let (reply, mut response) = oneshot::channel();
    assert!(
        queue
            .request(ControlRoundScope::peer(requested), Some(reply))
            .is_none()
    );
    assert!(queue.request(ControlRoundScope::all(), None).is_none());
    assert!(queue.complete(active.id(), true).is_empty());

    // When
    let pending = queue.take_pending().ok_or("pending round missing")?;
    let selected = ControlRoundRequest {
        local_endpoint_id: local,
        rotation: pending.rotation(),
        now_ms: 0,
        scope: pending.scope().clone(),
    }
    .select(&eligible);

    // Then
    assert!(selected.contains(&requested));
    assert!(matches!(
        response.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert_eq!(queue.complete(pending.id(), true).len(), 1);
    let global = queue.take_pending().ok_or("global round missing")?;
    let global_selected = ControlRoundRequest {
        local_endpoint_id: local,
        rotation: global.rotation(),
        now_ms: 0,
        scope: global.scope().clone(),
    }
    .select(&eligible);
    assert_eq!(global_selected.len(), 4);
    Ok(())
}

#[test]
fn every_control_trigger_maps_to_the_expected_round_scope() -> TestResult {
    // Given
    let peer = EndpointSecret::parse(&[0x50; 32])?.endpoint_id();
    let global = [
        ControlRoundTrigger::Startup,
        ControlRoundTrigger::ManifestAdvanced,
        ControlRoundTrigger::AddressAdvanced,
        ControlRoundTrigger::RelayAdvanced,
        ControlRoundTrigger::EnrollmentCompleted,
        ControlRoundTrigger::Periodic,
    ];

    // When
    let scopes = global.map(ControlRoundTrigger::scope);
    let explicit = ControlRoundTrigger::Explicit(ControlRoundScope::peer(peer)).scope();

    // Then
    assert!(
        scopes
            .into_iter()
            .all(|scope| scope == ControlRoundScope::all())
    );
    assert_eq!(explicit, ControlRoundScope::peer(peer));
    Ok(())
}

#[test]
fn accepted_control_changes_emit_only_the_matching_advancement_triggers() {
    // Given
    let changes = ControlChanges {
        manifest: true,
        address: false,
        relay: true,
    };

    // When
    let triggers = changes.triggers().collect::<Vec<_>>();

    // Then
    assert_eq!(
        triggers,
        vec![
            ControlRoundTrigger::ManifestAdvanced,
            ControlRoundTrigger::RelayAdvanced,
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_peer_without_a_prepared_exchange_is_not_reported_as_synchronized() -> TestResult {
    // Given
    let state = TempState::new("missing-peer")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let handle = runtime.handle();
    handle
        .observe_memberships(vec![SpaceId::derive(
            b"observed space without address state",
        )])
        .await?;
    let requested = EndpointSecret::parse(&[0x70; 32])?.endpoint_id();

    // When
    let result =
        tokio::time::timeout(Duration::from_secs(5), handle.sync_control_with(requested)).await?;
    runtime.shutdown().await?;

    // Then
    assert!(result.is_err());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_peer_without_any_membership_is_not_reported_as_synchronized() -> TestResult {
    // Given
    let state = TempState::new("empty-membership")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let requested = EndpointSecret::parse(&[0x71; 32])?.endpoint_id();

    // When
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.handle().sync_control_with(requested),
    )
    .await?;
    runtime.shutdown().await?;

    // Then
    assert!(result.is_err());
    Ok(())
}
