//! Durable Agents and tasks: definitions, `RequestId` binding, assignment, cancellation,
//! instance conclusion, and limits.

#[path = "common/support.rs"]
mod support;

#[path = "common/agent_fixture.rs"]
mod fixture;

use fixture::{agent, created, definition, endpoint, new_task, ready_agent, text};
use ma2a_core::service::{
    AgentEventBody, AgentId, AgentTaskReason, AgentTaskState, MessageRole, ProcessId,
};
use ma2a_store::{
    AgentDefine, AgentDelivery, AgentTaskChange, AgentTaskCreation, AgentTaskFilter, LocalExposure,
    MAX_LIVE_TASKS_PER_CREATOR, Repository, StoreConfig,
};
use support::{TempState, TestResult};

fn open(name: &str) -> support::TestResultValue<(TempState, Repository)> {
    let state = TempState::new(name)?;
    let repository = Repository::open(&StoreConfig::new(state.path()))?;
    Ok((state, repository))
}

fn state_of(repository: &Repository, seed: u8) -> support::TestResultValue<AgentTaskState> {
    let task = ma2a_core::service::AgentTaskId::from_bytes([seed; 16]);
    Ok(repository
        .agent_task(task)?
        .ok_or("missing task")?
        .summary
        .state)
}

#[test]
fn a_definition_changes_only_while_its_agent_is_stopped() -> TestResult {
    let (_state, mut repository) = open("agent-define")?;
    let first = repository.define_agent(&definition(4)?, 1)?.into_value();
    assert!(matches!(first, AgentDefine::Defined(_, true)));
    let mut changed = definition(2)?;
    changed.description = "Answers fewer questions.".to_owned();
    let second = repository.define_agent(&changed, 2)?.into_value();
    let AgentDefine::Defined(record, false) = second else {
        return Err(format!("expected an update, got {second:?}").into());
    };
    assert_eq!(record.definition, changed);
    repository.set_agent_enabled(&agent()?, (true, 3))?;
    assert_eq!(
        repository.define_agent(&definition(4)?, 4)?.into_value(),
        AgentDefine::Active
    );
    assert_eq!(
        repository.undefine_agent(&agent()?)?.into_value(),
        AgentDefine::Active
    );
    repository.set_agent_enabled(&agent()?, (false, 5))?;
    assert!(matches!(
        repository.undefine_agent(&agent()?)?.into_value(),
        AgentDefine::Defined(..)
    ));
    assert_eq!(
        repository.undefine_agent(&agent()?)?.into_value(),
        AgentDefine::Missing
    );
    assert!(repository.agents()?.into_value().is_empty());
    Ok(())
}

#[test]
fn one_request_id_binds_exactly_one_task_per_creator() -> TestResult {
    let (_state, mut repository) = open("agent-binding")?;
    let caller = endpoint(1);
    assert_eq!(
        repository
            .create_agent_task(&new_task(1, caller)?)?
            .into_value(),
        AgentTaskCreation::UnknownAgent
    );
    repository.define_agent(&definition(4)?, 1)?;
    assert_eq!(
        repository
            .create_agent_task(&new_task(1, caller)?)?
            .into_value(),
        AgentTaskCreation::Unavailable
    );
    repository.set_agent_enabled(&agent()?, (true, 2))?;
    let id = created(
        &repository
            .create_agent_task(&new_task(1, caller)?)?
            .into_value(),
    )?;
    // The same submission again answers the same task; nothing new is recorded.
    let mut retry = new_task(1, caller)?;
    retry.id = ma2a_core::service::AgentTaskId::from_bytes([9; 16]);
    let existing = repository.create_agent_task(&retry)?.into_value();
    let AgentTaskCreation::Existing(record) = existing else {
        return Err(format!("expected the existing task, got {existing:?}").into());
    };
    assert_eq!(record.summary.id, id);
    assert_eq!(record.summary.state, AgentTaskState::Queued);
    retry.fingerprint = [0xee; 32];
    assert_eq!(
        repository.create_agent_task(&retry)?.into_value(),
        AgentTaskCreation::Conflict
    );
    // Another creator may use the same RequestId for its own task.
    let mut other = new_task(2, endpoint(2))?;
    other.binding.1 = fixture::request(1)?;
    created(&repository.create_agent_task(&other)?.into_value())?;
    let events = repository.agent_task_events(id, (0, 10))?;
    assert_eq!(events.len(), 1);
    assert_eq!(
        events.first().map(|event| &event.body),
        Some(&AgentEventBody::State(AgentTaskState::Queued, None))
    );
    Ok(())
}

#[test]
fn unfinished_tasks_per_creator_are_limited_without_effect() -> TestResult {
    let (_state, mut repository) = open("agent-limits")?;
    repository.define_agent(&definition(4)?, 1)?;
    repository.set_agent_enabled(&agent()?, (true, 2))?;
    let limit = u8::try_from(MAX_LIVE_TASKS_PER_CREATOR)?;
    for seed in 1..=limit {
        created(
            &repository
                .create_agent_task(&new_task(seed, endpoint(1))?)?
                .into_value(),
        )?;
    }
    assert_eq!(
        repository
            .create_agent_task(&new_task(limit + 1, endpoint(1))?)?
            .into_value(),
        AgentTaskCreation::LimitExceeded
    );
    created(
        &repository
            .create_agent_task(&new_task(limit + 2, endpoint(2))?)?
            .into_value(),
    )?;
    Ok(())
}

#[test]
fn ready_instances_take_queued_tasks_oldest_first_up_to_their_slots() -> TestResult {
    let (_state, mut repository) = open("agent-assign")?;
    repository.define_agent(&definition(2)?, 1)?;
    repository.set_agent_enabled(&agent()?, (true, 2))?;
    for seed in 1..=3 {
        repository.create_agent_task(&new_task(seed, endpoint(1))?)?;
    }
    // Nothing is assigned before an instance is ready.
    assert!(
        repository
            .assign_agent_tasks(&agent()?, 3)?
            .into_value()
            .is_empty()
    );
    let generation = {
        let record = repository
            .begin_agent_instance(&agent()?, 4)?
            .into_value()
            .ok_or("no instance")?;
        record.instance.ok_or("no instance")?.generation
    };
    assert!(
        repository
            .assign_agent_tasks(&agent()?, 5)?
            .into_value()
            .is_empty()
    );
    fixture::apply(
        &mut repository,
        (generation, 1),
        ma2a_store::AgentOutputEffect::Ready,
    )?;
    let assigned = repository.assign_agent_tasks(&agent()?, 6)?.into_value();
    assert_eq!(assigned.len(), 2);
    assert_eq!(state_of(&repository, 1)?, AgentTaskState::Running);
    assert_eq!(state_of(&repository, 2)?, AgentTaskState::Running);
    assert_eq!(state_of(&repository, 3)?, AgentTaskState::Queued);
    let outbox = repository.agent_outbox((&agent()?, generation))?;
    let deliveries: Vec<_> = outbox.iter().map(|entry| entry.delivery.clone()).collect();
    assert_eq!(
        deliveries,
        assigned
            .iter()
            .map(|task| AgentDelivery::Task(*task))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        outbox.iter().map(|entry| entry.seq).collect::<Vec<_>>(),
        [1, 2]
    );
    // A second begin while the instance is live does nothing.
    assert!(
        repository
            .begin_agent_instance(&agent()?, 7)?
            .into_value()
            .is_none()
    );
    Ok(())
}

#[test]
fn messages_sent_while_queued_are_delivered_with_the_assignment() -> TestResult {
    let (_state, mut repository) = open("agent-send")?;
    repository.define_agent(&definition(1)?, 1)?;
    repository.set_agent_enabled(&agent()?, (true, 2))?;
    let id = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    let more = text(MessageRole::User, "more context")?;
    let sent = repository.send_agent_message(id, (&more, 3))?.into_value();
    assert!(matches!(sent, AgentTaskChange::Changed(_)));
    let generation = ready_agent_again(&mut repository)?;
    repository.assign_agent_tasks(&agent()?, 4)?;
    let deliveries: Vec<_> = repository
        .agent_outbox((&agent()?, generation))?
        .into_iter()
        .map(|entry| entry.delivery)
        .collect();
    assert_eq!(
        deliveries,
        [AgentDelivery::Task(id), AgentDelivery::Message(id, 2)]
    );
    // A live task gets each later message delivered at once.
    repository.send_agent_message(id, (&more, 5))?;
    assert_eq!(repository.agent_outbox((&agent()?, generation))?.len(), 3);
    Ok(())
}

/// Starts the already-defined, already-enabled Agent's first instance and readies it.
fn ready_agent_again(repository: &mut Repository) -> support::TestResultValue<u64> {
    let record = repository
        .begin_agent_instance(&agent()?, 3)?
        .into_value()
        .ok_or("no instance")?;
    let generation = record.instance.ok_or("no instance")?.generation;
    fixture::apply(
        repository,
        (generation, 1),
        ma2a_store::AgentOutputEffect::Ready,
    )?;
    Ok(generation)
}

#[test]
fn cancellation_ends_queued_tasks_at_once_and_live_ones_after_their_instance_or_grace() -> TestResult
{
    let (_state, mut repository) = open("agent-cancel")?;
    let generation = ready_agent(&mut repository, 1)?;
    let running = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    let queued = created(
        &repository
            .create_agent_task(&new_task(2, endpoint(1))?)?
            .into_value(),
    )?;
    repository.assign_agent_tasks(&agent()?, 20)?;
    let requested = AgentTaskReason::Requested;
    repository.request_agent_cancel(queued, (requested, 21))?;
    assert_eq!(state_of(&repository, 2)?, AgentTaskState::Cancelled);
    let change = repository
        .request_agent_cancel(running, (requested, 22))?
        .into_value();
    let AgentTaskChange::Changed(record) = change else {
        return Err(format!("expected a requested cancel, got {change:?}").into());
    };
    assert_eq!(record.summary.state, AgentTaskState::Running);
    assert_eq!(record.cancel, Some((requested, 22)));
    let last = repository
        .agent_outbox((&agent()?, generation))?
        .pop()
        .map(|entry| entry.delivery);
    assert_eq!(last, Some(AgentDelivery::Cancel(running)));
    // The submitter can no longer send to a task being cancelled.
    let more = text(MessageRole::User, "late")?;
    assert!(matches!(
        repository
            .send_agent_message(running, (&more, 23))?
            .into_value(),
        AgentTaskChange::Ended(_)
    ));
    assert!(
        repository
            .expire_agent_cancels((22 + 9_999, 10_000))?
            .into_value()
            .is_empty()
    );
    assert_eq!(
        repository
            .expire_agent_cancels((22 + 10_000, 10_000))?
            .into_value(),
        [running]
    );
    let record = repository.agent_task(running)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Cancelled);
    assert_eq!(record.summary.reason, Some(requested));
    Ok(())
}

#[test]
fn a_concluded_instance_fails_what_it_held_and_keeps_the_queue() -> TestResult {
    let (_state, mut repository) = open("agent-lost")?;
    let generation = ready_agent(&mut repository, 1)?;
    let held = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    let waiting = created(
        &repository
            .create_agent_task(&new_task(2, endpoint(1))?)?
            .into_value(),
    )?;
    repository.assign_agent_tasks(&agent()?, 20)?;
    let ended = repository
        .end_agent_instance((&agent()?, generation), 30)?
        .into_value();
    assert_eq!(ended, [held]);
    let record = repository.agent_task(held)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Failed);
    assert_eq!(record.summary.reason, Some(AgentTaskReason::AgentLost));
    assert_eq!(state_of(&repository, 2)?, AgentTaskState::Queued);
    assert!(repository.agent_outbox((&agent()?, generation))?.is_empty());
    // Concluding twice changes nothing; the next instance takes the next generation.
    assert!(
        repository
            .end_agent_instance((&agent()?, generation), 31)?
            .into_value()
            .is_empty()
    );
    let next = repository
        .begin_agent_instance(&agent()?, 32)?
        .into_value()
        .ok_or("no instance")?;
    let instance = next.instance.ok_or("no instance")?;
    assert_eq!(instance.generation, generation + 1);
    assert!(instance.process_id.is_none() && !instance.ready);
    assert!(
        repository
            .bind_agent_process(
                (&agent()?, instance.generation),
                ProcessId::from_bytes([7; 16])
            )?
            .into_value()
    );
    assert!(
        !repository
            .bind_agent_process(
                (&agent()?, instance.generation),
                ProcessId::from_bytes([8; 16])
            )?
            .into_value()
    );
    // Stopping the Agent fails everything it has not finished.
    repository.set_agent_enabled(&agent()?, (false, 33))?;
    let record = repository.agent_task(waiting)?.ok_or("missing")?;
    assert_eq!(record.summary.reason, Some(AgentTaskReason::AgentStopped));
    Ok(())
}

#[test]
fn tasks_are_listed_per_creator_and_agent() -> TestResult {
    let (_state, mut repository) = open("agent-list")?;
    repository.define_agent(&definition(4)?, 1)?;
    repository.set_agent_enabled(&agent()?, (true, 2))?;
    for (seed, creator) in [(1, 1), (2, 2), (3, 1)] {
        repository.create_agent_task(&new_task(seed, endpoint(creator))?)?;
    }
    let mine = repository
        .agent_tasks(&AgentTaskFilter {
            creator: Some(endpoint(1)),
            agent: Some(agent()?),
            after: None,
            limit: 10,
        })?
        .into_value();
    assert_eq!(mine.len(), 2);
    let none = repository
        .agent_tasks(&AgentTaskFilter {
            creator: None,
            agent: Some(AgentId::new("other")?),
            after: None,
            limit: 10,
        })?
        .into_value();
    assert!(none.is_empty());
    assert_eq!(repository.live_agent_tasks()?.len(), 3);
    Ok(())
}

#[test]
fn agent_exposures_name_one_agent_per_space() -> TestResult {
    let (_state, mut repository) = open("agent-exposure")?;
    let member = ma2a_core::SpaceMemberV1::new(
        endpoint(0x61),
        "owner".to_owned(),
        ma2a_core::MemberCapabilities::new(true, true),
    )?;
    let space = repository
        .create_owned_space(&ma2a_store::SpaceCreation::new(
            1_700_000_000_000,
            member,
            ma2a_core::SpacePolicyV1::phase_one_default(),
        ))?
        .space_id();
    repository.expose_service(space, &LocalExposure::Agent(agent()?))?;
    repository.expose_service(space, &LocalExposure::Agent(AgentId::new("second")?))?;
    let views = repository.service_exposure_views()?;
    let agents = views.first().map(|view| view.agents().to_vec());
    assert_eq!(agents, Some(vec![agent()?, AgentId::new("second")?]));
    assert!(
        repository
            .unexpose_service(space, &LocalExposure::Agent(agent()?))?
            .into_value()
    );
    let listed: Vec<_> = repository
        .service_exposures()?
        .into_value()
        .into_iter()
        .map(|record| record.exposure)
        .collect();
    assert_eq!(listed, [LocalExposure::Agent(AgentId::new("second")?)]);
    Ok(())
}

#[test]
fn finished_tasks_beyond_the_retention_bound_make_room_oldest_first() -> TestResult {
    let (_state, mut repository) = open("agent-retention")?;
    repository.define_agent(&definition(4)?, 1)?;
    let bound = ma2a_store::MAX_RETAINED_TASKS_PER_CREATOR;
    let rounds = bound / MAX_LIVE_TASKS_PER_CREATOR + 1;
    let mut seed = 0_u8;
    for round in 0..rounds {
        repository.set_agent_enabled(&agent()?, (true, 100 + u64::from(round)))?;
        for _ in 0..MAX_LIVE_TASKS_PER_CREATOR {
            seed += 1;
            created(
                &repository
                    .create_agent_task(&new_task(seed, endpoint(1))?)?
                    .into_value(),
            )?;
        }
        // Stopping fails every unfinished task, which makes them retained history.
        repository.set_agent_enabled(&agent()?, (false, 200 + u64::from(round)))?;
    }
    repository.set_agent_enabled(&agent()?, (true, 300))?;
    created(
        &repository
            .create_agent_task(&new_task(seed + 1, endpoint(1))?)?
            .into_value(),
    )?;
    let finished = repository
        .agent_tasks(&AgentTaskFilter {
            creator: Some(endpoint(1)),
            agent: None,
            after: None,
            limit: 1_000,
        })?
        .into_value()
        .into_iter()
        .filter(|record| record.summary.state.is_terminal())
        .count();
    assert_eq!(finished, usize::try_from(bound)?);
    // The oldest finished task and its binding are gone.
    assert!(
        repository
            .agent_task(ma2a_core::service::AgentTaskId::from_bytes([1; 16]))?
            .is_none()
    );
    Ok(())
}

#[test]
fn stopping_an_agent_ends_its_instance_so_a_restart_begins_a_new_one() -> TestResult {
    let (_state, mut repository) = open("agent-stop-instance")?;
    let generation = ready_agent(&mut repository, 1)?;
    let stopped = repository
        .set_agent_enabled(&agent()?, (false, 30))?
        .into_value()
        .ok_or("missing Agent")?;
    let instance = stopped.instance.ok_or("no instance")?;
    assert_eq!(instance.ended_ms, Some(30));
    // The stopped instance's Process is still named, so the caller can terminate it.
    assert_eq!(instance.process_id, Some(ProcessId::from_bytes([0x44; 16])));
    assert!(repository.agent_outbox((&agent()?, generation))?.is_empty());

    repository.set_agent_enabled(&agent()?, (true, 31))?;
    let next = repository
        .begin_agent_instance(&agent()?, 32)?
        .into_value()
        .ok_or("no new instance")?
        .instance
        .ok_or("no instance")?;
    assert_eq!(next.generation, generation + 1);
    // A launch racing the stop finds nothing to bind.
    repository.set_agent_enabled(&agent()?, (false, 33))?;
    assert!(
        !repository
            .bind_agent_process((&agent()?, next.generation), ProcessId::from_bytes([9; 16]))?
            .into_value()
    );
    Ok(())
}
