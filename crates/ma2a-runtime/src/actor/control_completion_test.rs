use super::{Actor, maintenance::FaultPoint, relay_lifecycle_test::TempState};
use crate::{
    control_sync::{ControlChanges, ControlRoundOutcome, ControlRoundScope, ControlRoundTrigger},
    error::RuntimeError,
    state::{Connectivity, RuntimeStatus},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};
use ma2a_core::{MemberCapabilities, SpaceMemberV1, SpacePolicyV1};
use ma2a_net::{EndpointBindOptions, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::{Repository, SpaceCreation, StoreConfig, StoreError};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::mpsc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

async fn fixture(config: &StoreConfig) -> TestResult<(Actor, tokio::task::JoinHandle<()>)> {
    let backend = StoreBackend::open(config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let identity = store.initialize().await?;
    let lookup = SpaceAddressLookup::default();
    let (enrollment, enrollment_calls) = mpsc::channel(1);
    let (control, control_calls) = mpsc::channel(1);
    let (echo, echo_calls) = mpsc::channel(1);
    let map = store.load_relay_map(identity.endpoint_id, 100).await?;
    let endpoint = RuntimeEndpoint::bind_with_lookup(
        identity.secret,
        lookup.clone(),
        EndpointBindOptions::new(enrollment, None)
            .with_control(control, false)
            .with_echo(echo)
            .with_relay_map(map.clone()),
    )
    .await?;
    let metrics = endpoint.echo_metrics();
    let state = RuntimeStatus {
        endpoint_id: identity.endpoint_id,
        endpoint_addr: endpoint.endpoint_addr(),
        endpoint_data: endpoint.endpoint_data()?,
        boot_id: [0x57; 16],
        revision: store.revision().await?,
        memberships: BTreeSet::new(),
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable: false,
        relay: crate::reachability::RelayReachabilityState::new(map),
    };
    let (actor, _, _) = Actor::new(
        state,
        endpoint,
        store,
        enrollment_calls,
        control_calls,
        echo_calls,
        metrics,
        lookup,
        Arc::new(crate::clock::SystemClock),
        None,
    );
    Ok((actor, task))
}

#[tokio::test]
async fn control_completion_failure_retains_followup_until_normal_maintenance() -> TestResult {
    let directory = TempState::new("control-completion")?;
    let config = StoreConfig::new(&directory.0);
    let (mut actor, store_task) = fixture(&config).await?;
    let member = SpaceMemberV1::new(
        actor.state.endpoint_id,
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = Repository::open(&config)?.create_owned_space(&SpaceCreation::new(
        10,
        member,
        SpacePolicyV1::phase_one_default(),
    ))?;
    let scheduled = actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no control round")?;
    actor.maintenance.fail_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    actor
        .finish_control_round(Ok((
            scheduled,
            Ok(Some(ControlRoundOutcome {
                revision: created.revision(),
                synchronized_peers: BTreeSet::new(),
                changes: ControlChanges {
                    manifest: true,
                    address: false,
                    relay: false,
                },
            })),
        )))
        .await?;
    // The next ordinary maintenance pass must finish the failed projection,
    // without another incoming control artifact or another mutation.
    actor.reconcile_membership_completion().await?;
    actor.refresh_relay_candidates().await?;
    actor.refresh_local_control_publications().await?;
    let triggers = actor.control_schedule_events.lock().unwrap().clone();
    assert!(triggers.contains(&ControlRoundTrigger::ManifestAdvanced));
    assert!(actor.state.memberships.contains(&created.space_id()));
    let count = triggers
        .iter()
        .filter(|trigger| **trigger == ControlRoundTrigger::ManifestAdvanced)
        .count();
    let revision = actor.state.revision;
    actor.reconcile_membership_completion().await?;
    assert_eq!(actor.state.revision, revision);
    assert_eq!(
        actor
            .control_schedule_events
            .lock()
            .unwrap()
            .iter()
            .filter(|trigger| **trigger == ControlRoundTrigger::ManifestAdvanced)
            .count(),
        count
    );
    actor.finish(false).await?;
    store_task.await?;
    Ok(())
}

#[tokio::test]
async fn control_integrity_failure_is_not_a_peer_rejection() -> TestResult {
    let directory = TempState::new("control-integrity")?;
    let config = StoreConfig::new(&directory.0);
    let (mut actor, store_task) = fixture(&config).await?;
    let scheduled = actor
        .control_queue
        .request(ControlRoundScope::all(), None)
        .ok_or("no round")?;
    let error = StoreError::SchemaMismatch {
        detail: "control regression integrity failure",
    };
    let result = actor
        .finish_control_round(Ok((scheduled, Err(error.into()))))
        .await;
    assert!(result.is_err());
    assert!(!result.unwrap_err().is_retryable_background());
    actor.finish(false).await?;
    store_task.await?;
    Ok(())
}

#[tokio::test]
async fn explicit_membership_observation_retains_failed_followup() -> TestResult {
    let directory = TempState::new("observation-completion")?;
    let config = StoreConfig::new(&directory.0);
    let (mut actor, store_task) = fixture(&config).await?;
    actor.maintenance.fail_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let result = actor.observe_memberships(Vec::new()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().committed_revision().is_some());
    assert!(actor.maintenance.membership_pending.is_some());
    actor.reconcile_membership_completion().await?;
    assert!(actor.maintenance.membership_pending.is_none());
    actor.finish(false).await?;
    store_task.await?;
    Ok(())
}
