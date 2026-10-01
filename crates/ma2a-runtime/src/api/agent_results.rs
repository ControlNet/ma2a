//! Results of the `agent_*` local API commands.

use base64::Engine as _;
use ma2a_core::service::{AgentEvent, AgentTaskDetails, AgentTaskSummary, ArtifactRef};
use serde_json::{Value, json};

use super::{
    agent_views::{
        AgentView, artifact_value, details_value, encoded_len, event_value, summary_value,
    },
    responses::{CommandResult, ResultKind},
};

/// Most Agents one `agent_list` page carries.
pub const MAX_AGENT_LIST_PAGE: usize = 16;
/// Most tasks one `agent_task_list` page carries.
pub const MAX_AGENT_TASK_PAGE: usize = 32;
/// Most events one `agent_task_events` page carries.
pub const MAX_AGENT_EVENT_PAGE: usize = 64;
/// Encoded size one page stays within, so a whole response fits one frame.
const MAX_PAGE_BYTES: usize = 49_152;

/// Keeps the longest prefix of `values` within [`MAX_PAGE_BYTES`], at least one; `true`
/// when anything was left out.
fn bounded(values: Vec<Value>) -> (Vec<Value>, bool) {
    let total = values.len();
    let mut used = 0;
    let kept: Vec<Value> = values
        .into_iter()
        .enumerate()
        .take_while(|(index, value)| {
            used += encoded_len(value).saturating_add(1);
            *index == 0 || used <= MAX_PAGE_BYTES
        })
        .map(|(_, value)| value)
        .collect();
    let cut = kept.len() < total;
    (kept, cut)
}

/// The result of one `agent_*` command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentResult {
    Agents(Vec<Value>, Option<String>),
    Defined(Value, bool),
    Undefined(String),
    Started(Value),
    Stopped(Value),
    Tasks(Vec<Value>, Option<String>),
    Task(Value),
    Events(Vec<Value>, bool),
    Artifact(Value),
    Submitted(Value, bool),
    Sent(Value),
    Cancelled(Value),
}

impl AgentResult {
    pub(super) const fn result_type(&self) -> &'static str {
        match self {
            Self::Agents(..) => "agents",
            Self::Defined(..) => "agent_defined",
            Self::Undefined(_) => "agent_undefined",
            Self::Started(_) => "agent_started",
            Self::Stopped(_) => "agent_stopped",
            Self::Tasks(..) => "agent_tasks",
            Self::Task(_) => "agent_task",
            Self::Events(..) => "agent_task_events",
            Self::Artifact(_) => "agent_task_artifact",
            Self::Submitted(..) => "agent_task_submitted",
            Self::Sent(_) => "agent_task_sent",
            Self::Cancelled(_) => "agent_task_cancelled",
        }
    }

    pub(super) fn to_value(&self) -> Value {
        match self {
            Self::Agents(agents, next) => json!({"agents": agents, "next": next}),
            Self::Defined(agent, created) => json!({"agent": agent, "created": created}),
            Self::Undefined(agent) => json!({"agent": agent}),
            Self::Tasks(tasks, next) => json!({"tasks": tasks, "next": next}),
            Self::Events(events, more) => json!({"events": events, "more": more}),
            Self::Submitted(task, existing) => json!({"task": task, "existing": existing}),
            Self::Started(value)
            | Self::Stopped(value)
            | Self::Task(value)
            | Self::Artifact(value)
            | Self::Sent(value)
            | Self::Cancelled(value) => value.clone(),
        }
    }
}

impl CommandResult {
    /// Creates one bounded `agents` page from up to one Agent more than a page; the page
    /// continues when that extra one exists or the page ran out of room.
    pub fn agents(mut agents: Vec<AgentView>) -> Self {
        let more = agents.len() > MAX_AGENT_LIST_PAGE;
        agents.truncate(MAX_AGENT_LIST_PAGE);
        let (values, cut) = bounded(agents.iter().map(AgentView::to_value).collect());
        let next = (more || cut)
            .then(|| {
                values
                    .len()
                    .checked_sub(1)
                    .and_then(|last| agents.get(last))
            })
            .flatten()
            .map(|agent| agent.id().to_owned());
        Self::agent(AgentResult::Agents(values, next))
    }

    /// Creates an `agent_defined` result; `created` when the Agent is new.
    pub fn agent_defined(agent: &AgentView, created: bool) -> Self {
        Self::agent(AgentResult::Defined(agent.to_value(), created))
    }

    /// Creates an `agent_undefined` result.
    pub fn agent_undefined(agent: &str) -> Self {
        Self::agent(AgentResult::Undefined(agent.to_owned()))
    }

    /// Creates an `agent_started` result.
    pub fn agent_started(agent: &AgentView) -> Self {
        Self::agent(AgentResult::Started(agent.to_value()))
    }

    /// Creates an `agent_stopped` result.
    pub fn agent_stopped(agent: &AgentView) -> Self {
        Self::agent(AgentResult::Stopped(agent.to_value()))
    }

    /// Creates one `agent_tasks` page from up to one task more than a page.
    pub fn agent_tasks(mut tasks: Vec<AgentTaskSummary>) -> Self {
        let more = tasks.len() > MAX_AGENT_TASK_PAGE;
        tasks.truncate(MAX_AGENT_TASK_PAGE);
        let next = more
            .then(|| tasks.last().map(|task| task.id.to_string()))
            .flatten();
        Self::agent(AgentResult::Tasks(
            tasks.iter().map(summary_value).collect(),
            next,
        ))
    }

    /// Creates an `agent_task` result.
    pub fn agent_task(details: &AgentTaskDetails) -> Self {
        Self::agent(AgentResult::Task(details_value(details)))
    }

    /// Creates one bounded `agent_task_events` page from up to one event more than a page.
    pub fn agent_task_events(mut events: Vec<AgentEvent>) -> Self {
        let more = events.len() > MAX_AGENT_EVENT_PAGE;
        events.truncate(MAX_AGENT_EVENT_PAGE);
        let (values, cut) = bounded(events.iter().map(event_value).collect());
        Self::agent(AgentResult::Events(values, more || cut))
    }

    /// Creates an `agent_task_artifact` chunk: `content` is the artifact from `offset`, and
    /// `eof` says it reaches the end.
    pub fn agent_task_artifact(artifact: &ArtifactRef, (offset, content): (u64, &[u8])) -> Self {
        let length = u64::try_from(content.len()).unwrap_or(u64::MAX);
        Self::agent(AgentResult::Artifact(json!({
            "artifact": artifact_value(artifact),
            "offset": offset,
            "content": base64::engine::general_purpose::STANDARD.encode(content),
            "eof": offset.saturating_add(length) >= artifact.size(),
        })))
    }

    /// Creates an `agent_task_submitted` result; `existing` when the request was a retry.
    pub fn agent_task_submitted(task: &AgentTaskSummary, existing: bool) -> Self {
        Self::agent(AgentResult::Submitted(summary_value(task), existing))
    }

    /// Creates an `agent_task_sent` result.
    pub fn agent_task_sent(task: &AgentTaskSummary) -> Self {
        Self::agent(AgentResult::Sent(summary_value(task)))
    }

    /// Creates an `agent_task_cancelled` result: the task as it is once cancellation was
    /// requested, which ends when its Agent confirms.
    pub fn agent_task_cancelled(task: &AgentTaskSummary) -> Self {
        Self::agent(AgentResult::Cancelled(summary_value(task)))
    }

    const fn agent(result: AgentResult) -> Self {
        Self(ResultKind::Agent(result), None)
    }
}
