//! Shared Agent Store fixtures: one started Agent with a ready instance.

use iroh_base::SecretKey;
use ma2a_core::{
    EndpointId, RequestId,
    service::{AgentId, AgentMessage, AgentTaskId, MessageRole, ProcessId},
};
use ma2a_store::{
    AgentDefinition, AgentOutput, AgentOutputApplied, AgentOutputEffect, AgentTaskCreation,
    NewAgentTask, Repository,
};

use super::support::TestResultValue as TestValue;

pub(crate) fn endpoint(seed: u8) -> EndpointId {
    SecretKey::from_bytes(&[seed; 32]).public().into()
}

pub(crate) fn request(seed: u8) -> TestValue<RequestId> {
    Ok(RequestId::try_from([seed; 16].as_slice())?)
}

pub(crate) fn agent() -> TestValue<AgentId> {
    Ok(AgentId::new("helper")?)
}

pub(crate) fn definition(max_tasks: u8) -> TestValue<AgentDefinition> {
    Ok(AgentDefinition {
        id: agent()?,
        description: "Answers questions.".to_owned(),
        start: b"canonical start".to_vec(),
        tools: 0b11,
        max_tasks,
    })
}

pub(crate) fn text(role: MessageRole, value: &str) -> TestValue<AgentMessage> {
    Ok(AgentMessage::text(role, value)?)
}

pub(crate) fn new_task(seed: u8, creator: EndpointId) -> TestValue<NewAgentTask> {
    Ok(NewAgentTask {
        id: AgentTaskId::from_bytes([seed; 16]),
        agent: agent()?,
        binding: (creator, request(seed)?),
        fingerprint: [seed; 32],
        from_agent: None,
        input: text(MessageRole::User, &format!("task {seed}"))?,
        created_ms: 1_000 + u64::from(seed),
    })
}

pub(crate) fn created(creation: &AgentTaskCreation) -> TestValue<AgentTaskId> {
    match creation {
        AgentTaskCreation::Created(record) => Ok(record.summary.id),
        other => Err(format!("expected a new task, got {other:?}").into()),
    }
}

/// Defines and starts the Agent and returns its first, ready instance generation.
pub(crate) fn ready_agent(repository: &mut Repository, max_tasks: u8) -> TestValue<u64> {
    repository.define_agent(&definition(max_tasks)?, 10)?;
    repository.set_agent_enabled(&agent()?, (true, 11))?;
    let record = repository
        .begin_agent_instance(&agent()?, 12)?
        .into_value()
        .ok_or("instance did not begin")?;
    let generation = record.instance.ok_or("no instance")?.generation;
    repository.bind_agent_process((&agent()?, generation), ProcessId::from_bytes([0x44; 16]))?;
    let ready = apply(repository, (generation, 1), AgentOutputEffect::Ready)?;
    assert_eq!(ready, AgentOutputApplied::Instance);
    Ok(generation)
}

/// Applies one adapter message that ends at `cursor`.
pub(crate) fn apply(
    repository: &mut Repository,
    (generation, cursor): (u64, u64),
    effect: AgentOutputEffect,
) -> TestValue<AgentOutputApplied> {
    let output = AgentOutput {
        agent: agent()?,
        generation,
        cursor,
        at_ms: 2_000 + cursor,
    };
    Ok(repository.apply_agent_output(&output, effect)?.into_value())
}
