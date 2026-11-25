//! An Agent instance acts only on its own tasks, and only while it is the current instance
//! of the definition that started it.

#[path = "common/support.rs"]
mod support;

#[path = "common/agent_fixture.rs"]
mod fixture;

use fixture::{agent, created, definition, endpoint, new_task, ready_agent};
use ma2a_core::service::{AgentId, AgentTaskId, AgentTaskState, ProcessId};
use ma2a_store::{
    AgentDefinition, AgentOutput, AgentOutputApplied, AgentOutputEffect, Repository, StoreConfig,
};
use support::{TempState, TestResult, TestResultValue as TestValue};

fn other() -> TestValue<AgentId> {
    Ok(AgentId::new("other")?)
}

/// Starts one instance of `id` and makes it ready; returns its generation.
fn begin_ready(repository: &mut Repository, id: &AgentId, at_ms: u64) -> TestValue<u64> {
    let generation = repository
        .begin_agent_instance(id, at_ms)?
        .into_value()
        .ok_or("instance did not begin")?
        .instance
        .ok_or("no instance")?
        .generation;
    repository.bind_agent_process((id, generation), ProcessId::from_bytes([0x55; 16]))?;
    let ready = output(repository, (id, generation), AgentOutputEffect::Ready)?;
    assert_eq!(ready, AgentOutputApplied::Instance);
    Ok(generation)
}

fn output(
    repository: &mut Repository,
    (id, generation): (&AgentId, u64),
    effect: AgentOutputEffect,
) -> TestValue<AgentOutputApplied> {
    let cursor = repository
        .agent(id)?
        .and_then(|record| record.instance)
        .map_or(0, |instance| instance.cursor)
        + 1;
    let output = AgentOutput {
        agent: id.clone(),
        generation,
        cursor,
        at_ms: 3_000 + cursor,
    };
    Ok(repository.apply_agent_output(&output, effect)?.into_value())
}

fn task_state(repository: &Repository, task: AgentTaskId) -> TestValue<AgentTaskState> {
    Ok(repository
        .agent_task(task)?
        .ok_or("missing task")?
        .summary
        .state)
}

/// Agent `other` running at the same generation as `helper`, whose task is running.
fn two_agents_at_one_generation(
    name: &str,
) -> TestValue<(TempState, Repository, u64, AgentTaskId)> {
    let state = TempState::new(name)?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.define_agent(
        &AgentDefinition {
            id: other()?,
            ..definition(4)?
        },
        5,
    )?;
    repository.set_agent_enabled(&other()?, (true, 6))?;
    let mut other_generation = begin_ready(&mut repository, &other()?, 7)?;
    let generation = ready_agent(&mut repository, 4)?;
    while other_generation < generation {
        repository.end_agent_instance((&other()?, other_generation), 8)?;
        other_generation = begin_ready(&mut repository, &other()?, 9)?;
    }
    assert_eq!(other_generation, generation);
    let task = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    repository.assign_agent_tasks(&agent()?, 20)?;
    assert_eq!(task_state(&repository, task)?, AgentTaskState::Running);
    Ok((state, repository, generation, task))
}

#[test]
fn another_agent_cannot_conclude_a_task_at_the_same_generation() -> TestResult {
    // Given
    let (_state, mut repository, generation, task) = two_agents_at_one_generation("cross-state")?;

    // When
    let applied = output(
        &mut repository,
        (&other()?, generation),
        AgentOutputEffect::State {
            task,
            state: AgentTaskState::Completed,
            message: None,
            error: None,
        },
    )?;

    // Then
    assert_eq!(applied, AgentOutputApplied::Ignored);
    assert_eq!(task_state(&repository, task)?, AgentTaskState::Running);
    Ok(())
}

#[test]
fn another_agent_cannot_call_tools_for_a_task_at_the_same_generation() -> TestResult {
    // Given
    let (_state, mut repository, generation, task) = two_agents_at_one_generation("cross-tool")?;

    // When
    let applied = output(
        &mut repository,
        (&other()?, generation),
        AgentOutputEffect::ToolCall {
            task,
            call: "c0".to_owned(),
            name: "exec.run".to_owned(),
            request: br#"{"program":"true","args":[]}"#.to_vec(),
            rejection: br#"{"error":{"code":"concurrency_exceeded","message":"limit"}}"#.to_vec(),
            ended: br#"{"error":{"code":"task_ended","message":"ended"}}"#.to_vec(),
        },
    )?;

    // Then
    assert_eq!(applied, AgentOutputApplied::Ignored);
    assert!(repository.tool_call((task, "c0"))?.is_none());
    Ok(())
}

#[test]
fn an_instance_of_a_removed_definition_cannot_end_its_recreated_successor() -> TestResult {
    // Given: the Agent is removed and defined again while its old Process still ends.
    let state = TempState::new("recreated")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let old_generation = ready_agent(&mut repository, 2)?;
    repository.set_agent_enabled(&agent()?, (false, 20))?;
    repository.undefine_agent(&agent()?)?;
    repository.define_agent(&definition(2)?, 21)?;
    repository.set_agent_enabled(&agent()?, (true, 22))?;
    let new_generation = begin_ready(&mut repository, &agent()?, 23)?;
    let task = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    repository.assign_agent_tasks(&agent()?, 30)?;

    // When
    let failed = repository
        .end_agent_instance((&agent()?, old_generation), 40)?
        .into_value();

    // Then
    assert!(new_generation > old_generation);
    assert!(failed.is_empty());
    assert_eq!(task_state(&repository, task)?, AgentTaskState::Running);
    Ok(())
}
