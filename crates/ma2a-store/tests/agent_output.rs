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
    MAX_OPEN_TOOL_CALLS_PER_TASK, MAX_PROGRESS_EVENTS, MAX_TOOL_CALLS_PER_TASK, Repository,
    StoreConfig,
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

/// The stored answer of a call its task's end left without a result.
const ENDED: &[u8] = ma2a_store::TASK_ENDED_ANSWER;

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
    // Acknowledging what was never sent prunes nothing.
    apply(
        &mut repository,
        (generation, cursor + 3),
        AgentOutputEffect::Ack(1_000),
    )?;
    assert_eq!(repository.agent_outbox((&agent()?, generation))?.len(), 3);
    // Acknowledging prunes every delivery up to that sequence number.
    apply(
        &mut repository,
        (generation, cursor + 4),
        AgentOutputEffect::Ack(2),
    )?;
    assert_eq!(repository.agent_outbox((&agent()?, generation))?.len(), 1);
    Ok(())
}

#[test]
fn the_call_past_the_limit_fails_the_task_and_answers_every_waiting_call() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-tool-limit")?;
    // One call stays open; every other one completes.
    let mut cursor = 10;
    for index in 0..MAX_TOOL_CALLS_PER_TASK {
        cursor += 1;
        let call = format!("c{index}");
        apply(&mut repository, (generation, cursor), tool(task, &call))?;
        if index > 0 {
            repository.complete_tool_call((task, &call), (b"{\"ok\":1}", 20))?;
        }
    }

    // When: the call past the limit.
    cursor += 1;
    let applied = apply(&mut repository, (generation, cursor), tool(task, "over"))?;

    // Then: the task failed, and neither the refused call nor the open one waits forever.
    let AgentOutputApplied::Task(record) = applied else {
        return Err(format!("the limit did not end the task: {applied:?}").into());
    };
    assert_eq!(record.summary.state, AgentTaskState::Failed);
    assert_eq!(record.summary.reason, Some(AgentTaskReason::Limit));
    for call in ["over", "c0"] {
        let answered = repository.tool_call((task, call))?.ok_or("missing call")?;
        assert_eq!(answered.result.as_deref(), Some(ENDED), "{call}");
    }
    let deliveries: Vec<_> = repository
        .agent_outbox((&agent()?, generation))?
        .into_iter()
        .map(|entry| entry.delivery)
        .filter(|delivery| matches!(delivery, AgentDelivery::ToolResult(..)))
        .collect();
    // Every other call's result was delivered when it completed; these two come last.
    assert_eq!(
        deliveries.get(deliveries.len().saturating_sub(2)..),
        Some(
            [
                AgentDelivery::ToolResult(task, "c0".to_owned()),
                AgentDelivery::ToolResult(task, "over".to_owned()),
            ]
            .as_slice()
        )
    );
    // The open call's own result, once it arrives, changes nothing.
    assert!(
        !repository
            .complete_tool_call((task, "c0"), (b"{\"ok\":2}", 30))?
            .into_value()
    );
    told_to_stop(&repository, (generation, task))
}

/// Asserts the call was answered as ended by its task's end, and that answer delivered.
fn answered_as_ended(
    repository: &Repository,
    (generation, task, call): (u64, AgentTaskId, &str),
) -> TestResult {
    let answered = repository.tool_call((task, call))?.ok_or("missing call")?;
    assert_eq!(answered.result.as_deref(), Some(ENDED), "{call}");
    let delivered = repository
        .agent_outbox((&agent()?, generation))?
        .into_iter()
        .any(|entry| entry.delivery == AgentDelivery::ToolResult(task, call.to_owned()));
    assert!(delivered, "{call} was not delivered");
    Ok(())
}

/// Asserts the instance holding the task was last sent its cancellation, so a handler waiting
/// in `ask` for a message that will never come returns.
fn told_to_stop(repository: &Repository, (generation, task): (u64, AgentTaskId)) -> TestResult {
    let last = repository
        .agent_outbox((&agent()?, generation))?
        .pop()
        .map(|entry| entry.delivery);
    assert_eq!(last, Some(AgentDelivery::Cancel(task)));
    Ok(())
}

#[test]
fn a_cancellation_its_instance_never_confirms_answers_the_calls_still_open() -> TestResult {
    // Given: a running task with a call still open, whose cancellation was requested.
    let (_state, mut repository, generation, task) = running("agent-cancel-grace")?;
    apply(&mut repository, (generation, 10), tool(task, "open"))?;
    repository.request_agent_cancel(task, (AgentTaskReason::Requested, 20))?;

    // When: the instance never confirms it within the grace period.
    let ended = repository
        .expire_agent_cancels((20 + 1_000, 1_000))?
        .into_value();

    // Then: the task ended as cancelled, and its open call is answered instead of left waiting.
    assert_eq!(ended, [task]);
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Cancelled);
    answered_as_ended(&repository, (generation, task, "open"))
}

#[test]
fn output_breaking_a_durable_bound_only_moves_the_cursor() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-unbound")?;
    let oversized = AgentOutputEffect::ToolCall {
        task,
        call: "big".to_owned(),
        name: "agent.submit".to_owned(),
        request: vec![b'x'; 16_385],
        rejection: b"{\"error\":\"concurrency_exceeded\"}".to_vec(),
    };
    let unnamed = AgentOutputEffect::ToolCall {
        task,
        call: "unnamed".to_owned(),
        name: String::new(),
        request: b"{}".to_vec(),
        rejection: b"{\"error\":\"concurrency_exceeded\"}".to_vec(),
    };
    for (cursor, effect) in [(11, oversized), (12, unnamed)] {
        assert_eq!(
            apply(&mut repository, (generation, cursor), effect)?,
            AgentOutputApplied::Ignored
        );
    }
    // The cursor moved past both, and the next line applies.
    assert_eq!(
        apply(&mut repository, (generation, 12), tool(task, "late"))?,
        AgentOutputApplied::Stale
    );
    assert_eq!(
        apply(&mut repository, (generation, 13), tool(task, "next"))?,
        AgentOutputApplied::ToolCall(task, "next".to_owned())
    );
    assert!(repository.tool_call((task, "big"))?.is_none());
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

#[test]
fn a_result_past_the_durable_event_bound_is_neither_kept_nor_reported_as_sent() -> TestResult {
    // Given: a task with room for no more durable events (`queued` and `running` hold two).
    let (_state, mut repository, generation, task) = running("agent-result-bound")?;
    let bound = u64::from(ma2a_store::MAX_DURABLE_EVENTS);
    for cursor in 0..bound - 2 {
        let note = AgentOutputEffect::Message(task, text(MessageRole::Agent, "note")?);
        apply(&mut repository, (generation, 10 + cursor), note)?;
    }

    // When: the Agent completes it with a result message, which would be the event past it.
    let completed = AgentOutputEffect::State {
        task,
        state: AgentTaskState::Completed,
        message: Some(text(MessageRole::Agent, "done")?),
        error: None,
    };
    apply(&mut repository, (generation, 10 + bound), completed)?;

    // Then: the task failed at its limit and holds no result.
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Failed);
    assert_eq!(record.summary.reason, Some(AgentTaskReason::Limit));
    assert_eq!(record.result, None);
    Ok(())
}

#[test]
fn a_message_sent_past_the_durable_event_bound_reports_the_task_ended() -> TestResult {
    // Given: a task with room for no more durable events.
    let (_state, mut repository, generation, task) = running("agent-send-bound")?;
    let bound = u64::from(ma2a_store::MAX_DURABLE_EVENTS);
    for cursor in 0..bound - 2 {
        let note = AgentOutputEffect::Message(task, text(MessageRole::Agent, "note")?);
        apply(&mut repository, (generation, 10 + cursor), note)?;
    }

    // When: the creator sends the message past it.
    let sent = repository
        .send_agent_message(task, (&text(MessageRole::User, "more")?, 900))?
        .into_value();

    // Then: the sender learns the task ended instead of that its message went through, and
    // the Agent waiting for that message is told to stop.
    assert!(matches!(sent, AgentTaskChange::Ended(_)), "{sent:?}");
    told_to_stop(&repository, (generation, task))
}

#[test]
fn state_changes_count_toward_the_durable_event_bound() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-state-bound")?;
    apply(&mut repository, (generation, 5), tool(task, "open"))?;
    let bound = u64::from(ma2a_store::MAX_DURABLE_EVENTS);
    for cursor in 0..bound {
        let to = if cursor % 2 == 0 {
            AgentTaskState::Waiting
        } else {
            AgentTaskState::Running
        };
        apply(&mut repository, (generation, 10 + cursor), state(task, to))?;
    }
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Failed);
    assert_eq!(record.summary.reason, Some(AgentTaskReason::Limit));
    assert!(record.summary.last_seq <= ma2a_store::MAX_DURABLE_EVENTS + 1);
    // The call still open when the task failed is answered, not left waiting.
    answered_as_ended(&repository, (generation, task, "open"))?;
    told_to_stop(&repository, (generation, task))
}

#[test]
fn an_instance_that_never_acknowledges_takes_no_more_tasks() -> TestResult {
    let directory = TempState::new("agent-unacked")?;
    let mut repository = Repository::open(&StoreConfig::new(directory.path()))?;
    let generation = ready_agent(&mut repository, 2)?;
    let mut tasks = Vec::new();
    for seed in 1..=2 {
        let creation = repository.create_agent_task(&new_task(seed, endpoint(1))?)?;
        tasks.push(created(&creation.into_value())?);
    }
    repository.assign_agent_tasks(&agent()?, 20)?;
    let [busy, other] = <[_; 2]>::try_from(tasks).map_err(|_| "two tasks")?;
    let unacked = u64::from(ma2a_store::MAX_UNACKED_DELIVERIES);
    for at_ms in 0..unacked - 2 {
        let message = text(MessageRole::User, "more")?;
        repository.send_agent_message(busy, (&message, 30 + at_ms))?;
    }
    let outbox = repository.agent_outbox((&agent()?, generation))?.len();
    assert_eq!(u64::try_from(outbox)?, unacked);
    // A slot frees up, but the instance has not read what it was sent.
    apply(
        &mut repository,
        (generation, 2),
        state(other, AgentTaskState::Completed),
    )?;
    let waiting = created(
        &repository
            .create_agent_task(&new_task(3, endpoint(1))?)?
            .into_value(),
    )?;
    assert!(
        repository
            .assign_agent_tasks(&agent()?, 40)?
            .into_value()
            .is_empty()
    );
    apply(
        &mut repository,
        (generation, 3),
        AgentOutputEffect::Ack(unacked),
    )?;
    assert_eq!(
        repository.assign_agent_tasks(&agent()?, 41)?.into_value(),
        [waiting]
    );
    Ok(())
}

#[test]
fn one_pass_stops_assigning_once_its_own_deliveries_reach_the_outbox_bound() -> TestResult {
    // Given: three free slots and three queued tasks, the first two holding 128 messages each.
    let directory = TempState::new("agent-unacked-pass")?;
    let mut repository = Repository::open(&StoreConfig::new(directory.path()))?;
    let generation = ready_agent(&mut repository, 3)?;
    let mut tasks = Vec::new();
    for seed in 1..=3 {
        let creation = repository.create_agent_task(&new_task(seed, endpoint(1))?)?;
        tasks.push(created(&creation.into_value())?);
    }
    let [first, second, third] = <[_; 3]>::try_from(tasks).map_err(|_| "three tasks")?;
    for (task, at_ms) in [(first, 100), (second, 300)] {
        for offset in 0..128 {
            let message = text(MessageRole::User, "more")?;
            repository.send_agent_message(task, (&message, at_ms + offset))?;
        }
    }

    // When: one pass assigns them.
    let assigned = repository.assign_agent_tasks(&agent()?, 500)?.into_value();

    // Then: the first two tasks bring 258 deliveries, so the third waits for acknowledgements.
    assert_eq!(assigned, [first, second]);
    let outbox = repository.agent_outbox((&agent()?, generation))?.len();
    assert_eq!(outbox, 258);
    apply(
        &mut repository,
        (generation, 2),
        AgentOutputEffect::Ack(258),
    )?;
    assert_eq!(
        repository.assign_agent_tasks(&agent()?, 501)?.into_value(),
        [third]
    );
    Ok(())
}

#[test]
fn an_artifact_over_the_event_bound_is_not_published() -> TestResult {
    let (_state, mut repository, generation, task) = running("agent-artifact-bound")?;
    apply(&mut repository, (generation, 5), tool(task, "publish"))?;
    // `queued` and `running` already hold two durable events.
    for cursor in 0..u64::from(ma2a_store::MAX_DURABLE_EVENTS) - 2 {
        let note = AgentOutputEffect::Message(task, text(MessageRole::Agent, "note")?);
        apply(&mut repository, (generation, 10 + cursor), note)?;
    }
    let late = artifact("late.txt", 1)?;
    let published = repository
        .publish_agent_artifact((task, ("publish", b"{\"ok\":1}")), (&late, 900))?
        .into_value();
    assert!(
        matches!(published, AgentTaskChange::Ended(_)),
        "{published:?}"
    );
    assert!(repository.agent_artifacts(task)?.is_empty());
    // The publishing call itself is answered: its task ended, so no later result reaches it.
    answered_as_ended(&repository, (generation, task, "publish"))?;
    told_to_stop(&repository, (generation, task))
}

/// A publication and the tool call's answer commit together, so a crash between them can
/// never leave a published artifact whose call runs again and conflicts with itself.
#[test]
fn an_artifact_publication_answers_its_tool_call_in_the_same_commit() -> TestResult {
    // Given
    let (_state, mut repository, generation, task) = running("agent-publish")?;
    assert_eq!(
        apply(&mut repository, (generation, 10), tool(task, "c0"))?,
        AgentOutputApplied::ToolCall(task, "c0".to_owned())
    );
    let report = artifact("report.txt", 1)?;

    // When
    let published = repository
        .publish_agent_artifact((task, ("c0", b"{\"ok\":\"published\"}")), (&report, 30))?
        .into_value();

    // Then
    assert!(matches!(published, AgentTaskChange::Changed(_)));
    let call = repository.tool_call((task, "c0"))?.ok_or("missing call")?;
    assert_eq!(
        call.result.as_deref(),
        Some(b"{\"ok\":\"published\"}".as_slice())
    );
    assert!(repository.pending_tool_calls()?.is_empty());
    assert!(
        repository
            .agent_outbox((&agent()?, generation))?
            .iter()
            .any(|entry| entry.delivery == AgentDelivery::ToolResult(task, "c0".to_owned()))
    );
    // The runner's own answer afterwards changes nothing.
    assert!(
        !repository
            .complete_tool_call((task, "c0"), (b"{\"ok\":2}", 31))?
            .into_value()
    );
    // A refused publication leaves the call for the runner to answer.
    assert_eq!(
        apply(&mut repository, (generation, 11), tool(task, "c1"))?,
        AgentOutputApplied::ToolCall(task, "c1".to_owned())
    );
    let refused = repository
        .publish_agent_artifact((task, ("c1", b"{}")), (&artifact("report.txt", 2)?, 32))?
        .into_value();
    assert_eq!(refused, AgentTaskChange::Refused);
    assert!(
        repository
            .tool_call((task, "c1"))?
            .ok_or("missing call")?
            .result
            .is_none()
    );
    Ok(())
}

#[test]
fn an_acknowledgement_past_every_sequence_number_only_moves_the_cursor() -> TestResult {
    // Given
    let (_state, mut repository, generation, task) = running("agent-huge-ack")?;

    // When: the Agent acknowledges a number no SQLite integer holds.
    let acked = apply(
        &mut repository,
        (generation, 10),
        AgentOutputEffect::Ack(u64::MAX),
    )?;

    // Then: it acknowledges nothing, and the next line applies.
    assert_eq!(acked, AgentOutputApplied::Ignored);
    assert_eq!(repository.agent_outbox((&agent()?, generation))?.len(), 1);
    assert!(matches!(
        apply(
            &mut repository,
            (generation, 11),
            state(task, AgentTaskState::Waiting)
        )?,
        AgentOutputApplied::Task(_)
    ));
    Ok(())
}

#[test]
fn a_state_carrying_a_user_message_is_ignored_whole() -> TestResult {
    // Given
    let (_state, mut repository, generation, task) = running("agent-user-state")?;

    // When: the Agent completes the task with a message in the user's role.
    let completed = AgentOutputEffect::State {
        task,
        state: AgentTaskState::Completed,
        message: Some(text(MessageRole::User, "wrong role")?),
        error: None,
    };
    let applied = apply(&mut repository, (generation, 10), completed)?;

    // Then: the invalid message voids the line, so the task is still running.
    assert_eq!(applied, AgentOutputApplied::Ignored);
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Running);
    Ok(())
}

#[test]
fn a_queued_task_at_its_durable_event_bound_fails_instead_of_running() -> TestResult {
    // Given: a queued task whose `queued` event and messages fill the durable event bound.
    let directory = TempState::new("agent-queued-bound")?;
    let mut repository = Repository::open(&StoreConfig::new(directory.path()))?;
    let generation = ready_agent(&mut repository, 1)?;
    let task = created(
        &repository
            .create_agent_task(&new_task(1, endpoint(1))?)?
            .into_value(),
    )?;
    for at_ms in 1..u64::from(ma2a_store::MAX_DURABLE_EVENTS) {
        let message = text(MessageRole::User, "more")?;
        repository.send_agent_message(task, (&message, 30 + at_ms))?;
    }
    // A second task waits behind it.
    let next = created(
        &repository
            .create_agent_task(&new_task(2, endpoint(1))?)?
            .into_value(),
    )?;

    // When: a slot is free, so the task would start running.
    let changed = repository.assign_agent_tasks(&agent()?, 900)?.into_value();

    // Then: `running` would be an event past the bound, so the task fails with `limit`, and
    // the slot it never took goes to the next task in the same pass.
    assert_eq!(changed, [task, next]);
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.summary.state, AgentTaskState::Failed);
    assert_eq!(record.summary.reason, Some(AgentTaskReason::Limit));
    assert_eq!(record.summary.last_seq, ma2a_store::MAX_DURABLE_EVENTS + 1);
    let next = repository.agent_task(next)?.ok_or("missing")?;
    assert_eq!(next.summary.state, AgentTaskState::Running);
    let outbox = repository.agent_outbox((&agent()?, generation))?;
    let deliveries: Vec<_> = outbox.into_iter().map(|entry| entry.delivery).collect();
    assert_eq!(deliveries, [AgentDelivery::Task(next.summary.id)]);
    Ok(())
}

#[test]
fn progress_its_event_could_not_be_read_back_is_ignored() -> TestResult {
    // Given
    let (_state, mut repository, generation, task) = running("agent-unreadable-progress")?;

    // When: the Agent reports progress with a NUL, or over 1 KiB in bytes but not in
    // characters, which the event log could store but never decode again.
    for (at, text) in [(10, "step\0one".to_owned()), (11, "é".repeat(600))] {
        let effect = AgentOutputEffect::Progress(task, text);
        let applied = apply(&mut repository, (generation, at), effect)?;

        // Then: the line changes nothing, and the task's events stay readable.
        assert_eq!(applied, AgentOutputApplied::Ignored);
    }
    assert!(repository.agent_task_events(task, (0, 64)).is_ok());
    let record = repository.agent_task(task)?.ok_or("missing")?;
    assert_eq!(record.progress, None);
    Ok(())
}
