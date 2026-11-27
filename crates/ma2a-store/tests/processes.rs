//! Durable Processes: `RequestId` binding, limits, guarded transitions, and retention.

#[path = "common/support.rs"]
mod support;

use iroh_base::SecretKey;
use ma2a_core::{
    EndpointId, ProtocolError, RequestId,
    service::{ProcessId, ProcessReason, ProcessState, Termination},
};
use ma2a_store::{
    ENDED_RETENTION_MS, LocalExposure, MAX_ACTIVE_PROCESSES, MAX_ACTIVE_PROCESSES_PER_CREATOR,
    MAX_RETAINED_PROCESSES_PER_CREATOR, NewProcess, ProcessArchive, ProcessCreation, ProcessFilter,
    ProcessTransition, Repository, StoreConfig, StoreError,
};
use rusqlite::Connection;
use support::{TempState, TestResult, TestResultValue as TestValue};

fn endpoint(seed: u8) -> EndpointId {
    SecretKey::from_bytes(&[seed; 32]).public().into()
}

fn request(seed: u8) -> TestValue<RequestId> {
    Ok(RequestId::try_from([seed; 16].as_slice())?)
}

const fn new_process(
    id: u8,
    (creator, request_id): (EndpointId, RequestId),
    fingerprint: u8,
) -> NewProcess<'static> {
    NewProcess {
        id: ProcessId::from_bytes([id; 16]),
        binding: (creator, request_id),
        fingerprint: [fingerprint; 32],
        start: (b"start bytes", Some("worker")),
        secret: [0x5a; 32],
        created_ms: 1_000,
    }
}

const fn step(to: ProcessState, at_ms: u64) -> ProcessTransition {
    ProcessTransition {
        to,
        reason: None,
        termination: None,
        at_ms,
    }
}

fn created(creation: &ProcessCreation) -> TestValue<ProcessId> {
    match creation {
        ProcessCreation::Created(record) => Ok(record.id),
        other => Err(format!("expected a new Process, got {other:?}").into()),
    }
}

#[test]
fn one_request_id_binds_exactly_one_process_per_creator() -> TestResult {
    let state = TempState::new("process-binding")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let binding = (endpoint(1), request(1)?);
    let first = repository.create_process(&new_process(1, binding, 7))?;
    let id = created(first.value())?;
    let retry = repository.create_process(&new_process(2, binding, 7))?;
    assert!(matches!(retry.value(), ProcessCreation::Existing(record) if record.id == id));
    assert_eq!(retry.revision(), first.revision());
    let changed = repository.create_process(&new_process(3, binding, 8))?;
    assert_eq!(changed.value(), &ProcessCreation::Conflict);
    let other = repository.create_process(&new_process(4, (endpoint(2), request(1)?), 7))?;
    assert_eq!(created(other.value())?, ProcessId::from_bytes([4; 16]));
    let record = repository.process(id)?.ok_or("missing Process")?;
    assert_eq!(record.state, ProcessState::Created);
    assert_eq!(record.secret, Some([0x5a; 32]));
    assert_eq!(record.start.as_deref(), Some(b"start bytes".as_slice()));
    assert!(
        repository
            .process(ProcessId::from_bytes([3; 16]))?
            .is_none()
    );
    Ok(())
}

#[test]
fn a_process_and_its_revision_are_read_together() -> TestResult {
    // Given: a Process another connection moves forward.
    let state = TempState::new("process-at")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let id = created(
        repository
            .create_process(&new_process(1, (endpoint(1), request(1)?), 7))?
            .value(),
    )?;
    let moved =
        Repository::open(&config)?.transition_process(id, step(ProcessState::Starting, 2))?;

    // When
    let read = repository.process_at(id)?;

    // Then: the record is the one that revision committed.
    assert_eq!(read.revision(), moved.revision());
    assert_eq!(
        read.value().as_ref().map(|record| record.state),
        Some(ProcessState::Starting)
    );
    assert_eq!(
        repository
            .process_at(ProcessId::from_bytes([9; 16]))?
            .value(),
        &None
    );
    Ok(())
}

#[test]
fn a_creator_limit_refuses_without_recording_anything() -> TestResult {
    let state = TempState::new("process-limit")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let limit = u8::try_from(MAX_ACTIVE_PROCESSES_PER_CREATOR)?;
    for seed in 0..limit {
        created(
            repository
                .create_process(&new_process(seed, (endpoint(1), request(seed)?), 1))?
                .value(),
        )?;
    }
    let before = repository.revision()?;
    let refused = repository.create_process(&new_process(99, (endpoint(1), request(99)?), 1))?;
    assert_eq!(refused.value(), &ProcessCreation::LimitExceeded);
    assert_eq!(repository.revision()?, before);
    let ended = ProcessId::from_bytes([0; 16]);
    repository.transition_process(ended, step(ProcessState::Starting, 2))?;
    repository.transition_process(
        ended,
        ProcessTransition {
            reason: Some(ProcessReason::SpawnFailed),
            ..step(ProcessState::Failed, 3)
        },
    )?;
    created(
        repository
            .create_process(&new_process(99, (endpoint(1), request(99)?), 1))?
            .value(),
    )?;
    Ok(())
}

#[test]
fn the_runtime_limit_refuses_a_creator_below_its_own_limit() -> TestResult {
    let state = TempState::new("process-global-limit")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let per_creator = MAX_ACTIVE_PROCESSES_PER_CREATOR;
    let creators = u8::try_from(MAX_ACTIVE_PROCESSES / per_creator)?;
    let mut seed = 0_u8;
    for creator in 1..=creators {
        for _ in 0..per_creator {
            let binding = (endpoint(creator), request(seed)?);
            created(
                repository
                    .create_process(&new_process(seed, binding, 1))?
                    .value(),
            )?;
            seed += 1;
        }
    }
    let newcomer = (endpoint(creators + 1), request(seed)?);
    let refused = repository.create_process(&new_process(seed, newcomer, 1))?;
    assert_eq!(refused.value(), &ProcessCreation::LimitExceeded);
    Ok(())
}

/// Records one Process for `creator` and ends it at once.
fn ended_process(repository: &mut Repository, (seed, creator): (u8, EndpointId)) -> TestResult {
    let id = created(
        repository
            .create_process(&new_process(seed, (creator, request(seed)?), 1))?
            .value(),
    )?;
    repository.transition_process(id, step(ProcessState::Starting, 2))?;
    repository.transition_process(
        id,
        ProcessTransition {
            termination: Some(Termination::Exited(0)),
            ..step(ProcessState::Exited, 3)
        },
    )?;
    Ok(())
}

#[test]
fn one_creator_ended_processes_never_lock_out_another_creator() -> TestResult {
    let state = TempState::new("process-retained")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let limit = u8::try_from(MAX_RETAINED_PROCESSES_PER_CREATOR)?;
    for seed in 0..limit {
        ended_process(&mut repository, (seed, endpoint(1)))?;
    }
    let refused = repository.create_process(&new_process(200, (endpoint(1), request(200)?), 1))?;
    assert_eq!(refused.value(), &ProcessCreation::LimitExceeded);
    // Another member still starts, and the first starts again once it deletes one.
    created(
        repository
            .create_process(&new_process(201, (endpoint(2), request(201)?), 1))?
            .value(),
    )?;
    repository.archive_process(ProcessId::from_bytes([0; 16]), 4)?;
    created(
        repository
            .create_process(&new_process(200, (endpoint(1), request(200)?), 1))?
            .value(),
    )?;
    Ok(())
}

#[test]
fn only_lifecycle_permitted_transitions_are_written() -> TestResult {
    let state = TempState::new("process-transitions")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let id = created(
        repository
            .create_process(&new_process(1, (endpoint(1), request(1)?), 1))?
            .value(),
    )?;
    let refused = repository.transition_process(id, step(ProcessState::Running, 2));
    assert!(
        matches!(refused, Err(StoreError::Protocol(error)) if error == ProtocolError::CONFLICT)
    );
    assert_eq!(
        repository.process(id)?.ok_or("missing")?.state,
        ProcessState::Created
    );
    repository.transition_process(id, step(ProcessState::Starting, 2))?;
    let running = repository.transition_process(id, step(ProcessState::Running, 3))?;
    assert_eq!(running.value().times.started_ms, Some(3));
    assert!(running.value().secret.is_some());
    let exited = repository.transition_process(
        id,
        ProcessTransition {
            termination: Some(Termination::Exited(4)),
            ..step(ProcessState::Exited, 9)
        },
    )?;
    let record = exited.into_value();
    assert_eq!(record.termination, Some(Termination::Exited(4)));
    assert_eq!(record.times.ended_ms, Some(9));
    assert_eq!(
        record.secret, None,
        "an ended Process forgets its supervisor secret"
    );
    assert!(
        repository
            .transition_process(id, step(ProcessState::Lost, 10))
            .is_err()
    );
    let missing = repository.transition_process(
        ProcessId::from_bytes([9; 16]),
        step(ProcessState::Starting, 1),
    );
    assert!(
        matches!(missing, Err(StoreError::Protocol(error)) if error == ProtocolError::NOT_FOUND)
    );
    Ok(())
}

#[test]
fn archived_tombstones_keep_the_binding_and_expire_on_schedule() -> TestResult {
    let state = TempState::new("process-archive")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let binding = (endpoint(1), request(1)?);
    let id = created(
        repository
            .create_process(&new_process(1, binding, 1))?
            .value(),
    )?;
    assert_eq!(
        repository.archive_process(id, 5)?.value(),
        &ProcessArchive::Active
    );
    repository.transition_process(id, step(ProcessState::Starting, 2))?;
    repository.transition_process(
        id,
        ProcessTransition {
            reason: Some(ProcessReason::SupervisorLost),
            ..step(ProcessState::Lost, 3)
        },
    )?;
    let archived = repository.archive_process(id, 4)?.into_value();
    assert!(
        matches!(&archived, ProcessArchive::Archived(record) if record.start.is_none() && record.is_archived())
    );
    let retry = repository.create_process(&new_process(2, binding, 1))?;
    assert!(matches!(retry.value(), ProcessCreation::Existing(record) if record.id == id));
    let listed = |archived| ProcessFilter {
        creator: Some(endpoint(1)),
        after: None,
        limit: 32,
        archived,
    };
    assert!(repository.processes(listed(false))?.value().is_empty());
    assert_eq!(repository.processes(listed(true))?.value().len(), 1);
    assert_eq!(
        repository
            .archive_process(ProcessId::from_bytes([8; 16]), 4)?
            .value(),
        &ProcessArchive::Missing
    );
    let other = created(
        repository
            .create_process(&new_process(3, (endpoint(1), request(3)?), 1))?
            .value(),
    )?;
    repository.transition_process(other, step(ProcessState::Failed, 10))?;
    assert!(
        repository
            .expire_processes(10 + ENDED_RETENTION_MS - 1)?
            .is_empty()
    );
    assert_eq!(
        repository.expire_processes(11 + ENDED_RETENTION_MS)?,
        vec![other]
    );
    assert!(repository.expire_processes(4 + 31 * 86_400_000)?.is_empty());
    assert!(
        repository.process(id)?.is_none(),
        "an expired tombstone is pruned"
    );
    Ok(())
}

#[test]
fn listings_filter_by_creator_and_continue_after_a_cursor() -> TestResult {
    let state = TempState::new("process-list")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    for seed in [3, 1, 2] {
        created(
            repository
                .create_process(&new_process(seed, (endpoint(1), request(seed)?), 1))?
                .value(),
        )?;
    }
    created(
        repository
            .create_process(&new_process(9, (endpoint(2), request(9)?), 1))?
            .value(),
    )?;
    let page = repository.processes(ProcessFilter {
        creator: Some(endpoint(1)),
        after: Some(ProcessId::from_bytes([1; 16])),
        limit: 1,
        archived: false,
    })?;
    let ids: Vec<_> = page.value().iter().map(|record| record.id).collect();
    assert_eq!(ids, [ProcessId::from_bytes([2; 16])]);
    let all = repository.processes(ProcessFilter {
        creator: None,
        after: None,
        limit: 32,
        archived: false,
    })?;
    assert_eq!(all.value().len(), 4);
    assert_eq!(repository.active_processes()?.len(), 4);
    Ok(())
}

#[test]
fn schema_seven_exposures_survive_the_process_migration() -> TestResult {
    let state = TempState::new("process-migration")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let space = support_space(&mut repository)?;
    repository.expose_service(space, &LocalExposure::Exec)?;
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch(
        "DELETE FROM schema_migrations WHERE version IN (12, 13); DROP TABLE departure_receipts; DELETE FROM schema_migrations WHERE version = 11; DROP TABLE hosted_services; DELETE FROM schema_migrations WHERE version = 10; DROP TABLE agent_outbox; DROP TABLE agent_tool_calls; DROP TABLE agent_artifacts; DROP TABLE agent_task_events; DROP TABLE agent_tasks; DROP TABLE agent_instances; DROP TABLE agents; DELETE FROM schema_migrations WHERE version = 9; DROP TABLE processes; DELETE FROM schema_migrations WHERE version = 8; PRAGMA user_version = 7;",
    )?;
    drop(connection);
    let mut reopened = Repository::open(&config)?;
    let kinds: Vec<_> = reopened
        .service_exposures()?
        .into_value()
        .into_iter()
        .map(|record| record.exposure)
        .collect();
    assert_eq!(kinds, [LocalExposure::Exec]);
    reopened.expose_service(space, &LocalExposure::Process)?;
    let views = reopened.service_exposure_views()?;
    assert!(views.iter().any(|view| view.process() && view.exec()));
    Ok(())
}

fn support_space(repository: &mut Repository) -> TestValue<ma2a_core::SpaceId> {
    let member = ma2a_core::SpaceMemberV1::new(
        endpoint(0x61),
        "owner".to_owned(),
        ma2a_core::MemberCapabilities::new(true, true),
    )?;
    Ok(repository
        .create_owned_space(&ma2a_store::SpaceCreation::new(
            1_700_000_000_000,
            member,
            ma2a_core::SpacePolicyV1::phase_one_default(),
        ))?
        .space_id())
}

#[test]
fn a_record_never_prints_its_secret_or_start() -> TestResult {
    let state = TempState::new("process-debug")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let creation = repository.create_process(&new_process(9, (endpoint(1), request(9)?), 1))?;
    let id = created(creation.value())?;
    let record = repository.process(id)?.ok_or("missing Process")?;
    let printed = format!("{record:?}");
    assert!(printed.contains("<redacted>"), "{printed}");
    assert!(!printed.contains("90, 90"), "{printed}");
    assert!(!printed.contains("start bytes"), "{printed}");
    Ok(())
}

#[test]
fn ended_processes_bound_to_unconcluded_agent_instances_are_not_archived() -> TestResult {
    use ma2a_core::service::AgentId;
    use ma2a_store::AgentDefinition;

    let state = TempState::new("process-agent-retention")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let local = endpoint(1);
    let process = created(
        repository
            .create_process(&new_process(1, (local, request(1)?), 1))?
            .value(),
    )?;
    repository.transition_process(process, step(ProcessState::Starting, 2))?;
    repository.transition_process(
        process,
        ProcessTransition {
            termination: Some(Termination::Exited(0)),
            ..step(ProcessState::Exited, 3)
        },
    )?;

    let agent = AgentId::new("worker")?;
    repository.define_agent(
        &AgentDefinition {
            id: agent.clone(),
            description: String::new(),
            start: b"start".to_vec(),
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("missing instance")?
        .generation;
    assert!(
        repository
            .bind_agent_process((&agent, generation), process)?
            .into_value()
    );

    // While the Agent instance is unconcluded, neither direct nor expiry archival removes it.
    assert_eq!(
        repository.archive_process(process, 4)?.value(),
        &ProcessArchive::Active
    );
    assert!(
        repository
            .expire_processes(3 + ENDED_RETENTION_MS + 1)?
            .is_empty()
    );

    // Once concluded, archival proceeds.
    repository.end_agent_instance((&agent, generation), 5)?;
    let archived = repository.archive_process(process, 6)?.into_value();
    assert!(matches!(archived, ProcessArchive::Archived(record) if record.is_archived()));
    Ok(())
}

#[test]
fn local_creation_checks_live_limits_before_archiving_and_reclaims_atomically() -> TestResult {
    use ma2a_core::service::AgentId;
    use ma2a_store::AgentDefinition;

    let state = TempState::new("process-local-create")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let local = endpoint(1);

    // One ended Process plus MAX_ACTIVE_PROCESSES_PER_CREATOR active Processes.
    ended_process(&mut repository, (1, local))?;
    let ended_id = ProcessId::from_bytes([1; 16]);
    let live_limit = u8::try_from(MAX_ACTIVE_PROCESSES_PER_CREATOR)?;
    for offset in 0..live_limit {
        created(
            repository
                .create_process(&new_process(10 + offset, (local, request(10 + offset)?), 1))?
                .value(),
        )?;
    }

    // Refused at live limit without archiving the ended Process.
    let refused = repository
        .create_local_process(&new_process(90, (local, request(90)?), 2))?
        .into_value();
    assert_eq!(refused, (ProcessCreation::LimitExceeded, None));
    assert!(
        !repository
            .process(ended_id)?
            .ok_or("missing")?
            .is_archived()
    );

    // Conclude the active Processes so retained limit is reached instead of live limit.
    for offset in 0..live_limit {
        let id = ProcessId::from_bytes([10 + offset; 16]);
        repository.transition_process(id, step(ProcessState::Starting, 3))?;
        repository.transition_process(
            id,
            ProcessTransition {
                termination: Some(Termination::Exited(0)),
                ..step(ProcessState::Exited, 4 + u64::from(offset))
            },
        )?;
    }
    let remaining = u8::try_from(MAX_RETAINED_PROCESSES_PER_CREATOR)? - 1 - live_limit;
    for offset in 0..remaining {
        let seed = 30 + offset;
        let id = created(
            repository
                .create_process(&new_process(seed, (local, request(seed)?), 1))?
                .value(),
        )?;
        repository.transition_process(id, step(ProcessState::Starting, 3))?;
        repository.transition_process(
            id,
            ProcessTransition {
                termination: Some(Termination::Exited(0)),
                ..step(ProcessState::Exited, 20 + u64::from(offset))
            },
        )?;
    }

    // Bind the oldest ended Process ([1; 16], ended_ms = 3) to an unconcluded Agent instance so
    // the next-oldest eligible Process ([10; 16], ended_ms = 4) is archived instead.
    let agent = AgentId::new("worker")?;
    repository.define_agent(
        &AgentDefinition {
            id: agent.clone(),
            description: String::new(),
            start: b"start".to_vec(),
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("missing instance")?
        .generation;
    repository.bind_agent_process((&agent, generation), ended_id)?;

    let (creation, archived) = repository
        .create_local_process(&new_process(91, (local, request(91)?), 50))?
        .into_value();
    let next_oldest = ProcessId::from_bytes([10; 16]);
    assert!(matches!(creation, ProcessCreation::Created(_)));
    assert_eq!(archived, Some(next_oldest));
    assert!(
        !repository
            .process(ended_id)?
            .ok_or("missing")?
            .is_archived()
    );
    assert!(
        repository
            .process(next_oldest)?
            .ok_or("missing")?
            .is_archived()
    );
    Ok(())
}
