//! Agent output applied exactly once, tool calls, artifacts, and task retention.

#[path = "common/support.rs"]
mod support;

#[path = "common/agent_fixture.rs"]
mod fixture;

use fixture::{agent, apply, created, endpoint, new_task, ready_agent, text};
use ma2a_core::service::{
    AgentEventBody, AgentTaskId, AgentTaskReason, AgentTaskState, ArtifactRef, MessageRole,
};
use ma2a_store::{
    AGENT_TASK_RETENTION_MS, AgentDelivery, AgentOutputApplied, AgentOutputEffect, AgentTaskChange,
    MAX_OPEN_TOOL_CALLS_PER_TASK, MAX_PROGRESS_EVENTS, Repository, StoreConfig,
};
use support::{TempState, TestResult, TestResultValue as TestValue};

/// A ready instance holding one running task.
fn running(name: &str) -> TestValue<(TempState, Repository, u64, AgentTaskId)> {
    let state = TempState::new(name)?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let generation = ready_agent(&mut repository, 4)?;
    let task = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    repository.assign_agent_tasks(&agent()?, 20)?;
    Ok((state, repository, generation, task))
}

const fn state(task: AgentTaskId, to: AgentTaskState) -> AgentOutputEffect {
    AgentOutputEffect::State {
        task,
        state: to,
        message: None,
        error: None,
    }
}

#[test]
fn every_message_applies_once_and_only_for_the_live_instance() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-once")?;
    let waiting = state(task, AgentTaskState::Waiting);
    assert!(matches!(
        apply(&mut repository, (generation, 10), waiting.clone())?,
        AgentOutputApplied::Task(_)
    ));
    // The same output position again, or an earlier one, is a replay and changes nothing.
    for cursor in [10, 5] {
        assert_eq!(
            apply(&mut repository, (generation, cursor), waiting.clone())?,
            AgentOutputApplied::Stale
        );
    }
    // Output of any other generation is never applied.
    assert_eq!(
        apply(
            &mut repository,
            (generation + 1, 99),
            state(task, AgentTaskState::Completed)
        )?,
        AgentOutputApplied::Stale
    );
    // An unknown task or an illegal transition only moves the cursor.
    let unknown = state(AgentTaskId::from_bytes([0x99; 16]), AgentTaskState::Running);
    assert_eq!(
        apply(&mut repository, (generation, 11), unknown)?,
        AgentOutputApplied::Ignored
    );
    // A cancellation nobody requested is not the Agent's to report.
    assert_eq!(
        apply(
            &mut repository,
            (generation, 12),
            state(task, AgentTaskState::Cancelled)
        )?,
        AgentOutputApplied::Ignored
    );
    let instance = repository
        .agent(&agent()?)?
        .and_then(|record| record.instance)
        .ok_or("no instance")?;
    assert_eq!(instance.cursor, 12);
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Waiting);
    Ok(())
}

#[test]
fn a_completed_task_keeps_its_inline_result_and_event_order() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-result")?;
    let note = text(MessageRole::Agent, "working on it")?;
    apply(
        &mut repository,
        (generation, 10),
        AgentOutputEffect::Message(task, note.clone()),
    )?;
    let result = text(MessageRole::Agent, "the answer is 42")?;
    let done = AgentOutputEffect::State {
        task,
        state: AgentTaskState::Completed,
        message: Some(result.clone()),
        error: None,
    };
    apply(&mut repository, (generation, 11), done)?;
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Completed);
    assert_eq!(record.result, Some(result.clone()));
    let bodies: Vec<_> = repository
        .agent_task_events(task, (0, 100))?
        .into_iter()
        .map(|event| event.body)
        .collect();
    assert_eq!(
        bodies,
        [
            AgentEventBody::State(AgentTaskState::Queued, None),
            AgentEventBody::State(AgentTaskState::Running, None),
            AgentEventBody::Message(note),
            AgentEventBody::Message(result),
            AgentEventBody::State(AgentTaskState::Completed, None),
        ]
    );
    // Reading after a sequence number resumes exactly there.
    let tail = repository.agent_task_events(task, (3, 100))?;
    assert_eq!(
        tail.iter().map(|event| event.seq).collect::<Vec<_>>(),
        [4, 5]
    );
    // A user message from the Agent's side is not the Agent's to write.
    let forged = AgentOutputEffect::Message(task, text(MessageRole::User, "x")?);
    assert_eq!(
        apply(&mut repository, (generation, 12), forged)?,
        AgentOutputApplied::Ignored
    );
    Ok(())
}

#[test]
fn only_the_newest_progress_is_retained_and_failure_keeps_its_text() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-progress")?;
    let total = MAX_PROGRESS_EVENTS + 10;
    for step in 0..total {
        let effect = AgentOutputEffect::Progress(task, format!("step {step}"));
        apply(&mut repository, (generation, 10 + u64::from(step)), effect)?;
    }
    let events = repository.agent_task_events(task, (0, 1_000))?;
    let progress = events
        .iter()
        .filter(|event| event.body.is_progress())
        .count();
    assert_eq!(progress, usize::try_from(MAX_PROGRESS_EVENTS)?);
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.progress.as_deref(), Some("step 73"));
    let failed = AgentOutputEffect::State {
        task,
        state: AgentTaskState::Failed,
        message: None,
        error: Some("upstream timeout".to_owned()),
    };
    apply(&mut repository, (generation, 1_000), failed)?;
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.reason, Some(AgentTaskReason::AgentFailed));
    assert_eq!(record.error.as_deref(), Some("upstream timeout"));
    Ok(())
}

fn tool(task: AgentTaskId, call: &str) -> AgentOutputEffect {
    AgentOutputEffect::ToolCall {
        task,
        call: call.to_owned(),
        name: "exec.run".to_owned(),
        request: b"{}".to_vec(),
        rejection: b"{\"error\":\"concurrency_exceeded\"}".to_vec(),
    }
}

#[test]
fn tool_calls_run_once_are_bounded_and_answer_their_instance() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-tools")?;
    let mut cursor = 10;
    for index in 0..MAX_OPEN_TOOL_CALLS_PER_TASK {
        cursor += 1;
        let applied = apply(
            &mut repository,
            (generation, cursor),
            tool(task, &format!("c{index}")),
        )?;
        assert_eq!(
            applied,
            AgentOutputApplied::ToolCall(task, format!("c{index}"))
        );
    }
    // The same call identifier never runs twice.
    assert_eq!(
        apply(&mut repository, (generation, cursor + 1), tool(task, "c0"))?,
        AgentOutputApplied::Ignored
    );
    // One more open call is answered at once with its rejection.
    assert_eq!(
        apply(
            &mut repository,
            (generation, cursor + 2),
            tool(task, "late")
        )?,
        AgentOutputApplied::ToolRejected(task, "late".to_owned())
    );
    assert_eq!(
        repository.pending_tool_calls()?.len(),
        usize::try_from(MAX_OPEN_TOOL_CALLS_PER_TASK)?
    );
    assert!(
        repository
            .complete_tool_call((task, "c0"), (b"{\"ok\":1}", 50))?
            .into_value()
    );
    assert!(
        !repository
            .complete_tool_call((task, "c0"), (b"{\"ok\":2}", 51))?
            .into_value()
    );
    let call = repository.tool_call((task, "c0"))?.ok_or("missing call")?;
    assert_eq!(call.result.as_deref(), Some(b"{\"ok\":1}".as_slice()));
    let deliveries: Vec<_> = repository
        .agent_outbox((&agent()?, generation))?
        .into_iter()
        .map(|entry| entry.delivery)
        .collect();
    assert_eq!(
        deliveries,
        [
            AgentDelivery::Task(task),
            AgentDelivery::ToolResult(task, "late".to_owned()),
            AgentDelivery::ToolResult(task, "c0".to_owned()),
        ]
    );
    // Acknowledging prunes every delivery up to that sequence number.
    apply(
        &mut repository,
        (generation, cursor + 3),
        AgentOutputEffect::Ack(2),
    )?;
    assert_eq!(repository.agent_outbox((&agent()?, generation))?.len(), 1);
    Ok(())
}

fn artifact(name: &str, byte: u8) -> TestValue<ArtifactRef> {
    Ok(ArtifactRef::new((name, "text/plain"), (3, [byte; 32]))?)
}

#[test]
fn artifacts_are_unique_per_task_and_expire_with_it() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-artifacts")?;
    let first = artifact("report.txt", 1)?;
    assert!(matches!(
        repository
            .add_agent_artifact(task, (&first, 30))?
            .into_value(),
        AgentTaskChange::Changed(_)
    ));
    assert_eq!(
        repository
            .add_agent_artifact(task, (&artifact("report.txt", 2)?, 31))?
            .into_value(),
        AgentTaskChange::Refused
    );
    for index in 1..16 {
        let named = artifact(&format!("part-{index}"), 3)?;
        repository.add_agent_artifact(task, (&named, 32))?;
    }
    assert_eq!(
        repository
            .add_agent_artifact(task, (&artifact("one-too-many", 4)?, 33))?
            .into_value(),
        AgentTaskChange::Refused
    );
    assert_eq!(repository.agent_artifacts(task)?.first(), Some(&first));
    assert_eq!(repository.referenced_artifact_hashes()?.len(), 2);
    let done = state(task, AgentTaskState::Completed);
    apply(&mut repository, (generation, 40), done)?;
    let ended = repository
        .agent_task(task)?
        .and_then(|record| record.summary.times.2)
        .ok_or("no end")?;
    assert_eq!(
        repository
            .expire_agent_tasks(ended + AGENT_TASK_RETENTION_MS - 1)?
            .into_value(),
        0
    );
    assert_eq!(
        repository
            .expire_agent_tasks(ended + AGENT_TASK_RETENTION_MS)?
            .into_value(),
        1
    );
    assert!(repository.agent_task(task)?.is_none());
    assert!(repository.referenced_artifact_hashes()?.is_empty());
    assert!(repository.agent_task_events(task, (0, 10))?.is_empty());
    Ok(())
}

#[test]
fn a_task_over_its_durable_event_bound_fails_with_limit() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-event-bound")?;
    let bound = u64::from(ma2a_store::MAX_DURABLE_EVENTS);
    // `queued` and `running` already hold two durable events.
    for cursor in 0..bound {
        let note = AgentOutputEffect::Message(task, text(MessageRole::Agent, "note")?);
        apply(&mut repository, (generation, 10 + cursor), note)?;
    }
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Failed);
    assert_eq!(record.summary.reason, Some(AgentTaskReason::Limit));
    Ok(())
}
