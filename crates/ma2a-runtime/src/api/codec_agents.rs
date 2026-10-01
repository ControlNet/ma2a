//! Local API commands for the Agents this Runtime hosts and their tasks.

use ma2a_core::{
    RequestId,
    service::{AgentId, AgentMessage, AgentTaskId, MAX_ARTIFACT_NAME_LEN},
};
use serde_json::{Map, Value, json};

use super::{
    ApiError,
    codec_agent_spec::{parse_spec, spec_value},
    codec_fields::{encode_hex, exact_fields, nullable, number, request_id, text},
    codec_messages::{message_value, parse_message},
    commands::CommandKind,
};
use crate::agents::AgentSpec;

/// One local operator command on hosted Agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentCommand {
    /// Store or replace one Agent's definition.
    Define(RequestId, Box<AgentSpec>),
    /// Remove one stopped Agent's definition.
    Undefine(RequestId, AgentId),
    /// Start one Agent.
    Start(RequestId, AgentId),
    /// Stop one Agent.
    Stop(RequestId, AgentId),
    /// Every Agent after this one.
    List(Option<AgentId>),
    /// Tasks of one Agent, or every Agent, after this one.
    Tasks(Option<AgentId>, Option<AgentTaskId>),
    /// One task in full.
    Inspect(AgentTaskId),
    /// Retained events of one task after this sequence number.
    Events(AgentTaskId, u32),
    /// A bounded chunk of one task's artifact from this offset.
    Artifact(AgentTaskId, String, u64),
    /// A new task from this Endpoint.
    Submit(RequestId, AgentId, AgentMessage),
    /// The next message of one task.
    Send(RequestId, AgentTaskId, AgentMessage),
    /// Request one task's cancellation without waiting for it to end.
    Cancel(RequestId, AgentTaskId),
}

impl AgentCommand {
    pub(crate) const fn operation(&self) -> &'static str {
        match self {
            Self::Define(..) => "agent_define",
            Self::Undefine(..) => "agent_undefine",
            Self::Start(..) => "agent_start",
            Self::Stop(..) => "agent_stop",
            Self::List(_) => "agent_list",
            Self::Tasks(..) => "agent_task_list",
            Self::Inspect(_) => "agent_task_inspect",
            Self::Events(..) => "agent_task_events",
            Self::Artifact(..) => "agent_task_artifact_read",
            Self::Submit(..) => "agent_task_submit",
            Self::Send(..) => "agent_task_send",
            Self::Cancel(..) => "agent_task_cancel",
        }
    }

    pub(crate) const fn request_id(&self) -> Option<RequestId> {
        match self {
            Self::List(_)
            | Self::Tasks(..)
            | Self::Inspect(_)
            | Self::Events(..)
            | Self::Artifact(..) => None,
            Self::Define(id, _)
            | Self::Undefine(id, _)
            | Self::Start(id, _)
            | Self::Stop(id, _)
            | Self::Submit(id, ..)
            | Self::Send(id, ..)
            | Self::Cancel(id, _) => Some(*id),
        }
    }

    pub(super) fn to_value(&self) -> Value {
        let operation = self.operation();
        let request = |id: &RequestId| encode_hex(id.as_bytes());
        match self {
            Self::Define(id, spec) => spec_value(operation, *id, spec),
            Self::Undefine(id, agent) | Self::Start(id, agent) | Self::Stop(id, agent) => {
                json!({"operation": operation, "request_id": request(id), "agent": agent.as_str()})
            }
            Self::List(after) => {
                json!({"operation": operation, "after": after.as_ref().map(AgentId::as_str)})
            }
            Self::Tasks(agent, after) => json!({
                "operation": operation,
                "agent": agent.as_ref().map(AgentId::as_str),
                "after": after.map(|task| task.to_string()),
            }),
            Self::Inspect(task) => json!({"operation": operation, "task_id": task.to_string()}),
            Self::Events(task, after) => {
                json!({"operation": operation, "task_id": task.to_string(), "after": after})
            }
            Self::Artifact(task, name, offset) => json!({
                "operation": operation,
                "task_id": task.to_string(),
                "name": name,
                "offset": offset,
            }),
            Self::Submit(id, agent, message) => json!({
                "operation": operation,
                "request_id": request(id),
                "agent": agent.as_str(),
                "message": message_value(message),
            }),
            Self::Send(id, task, message) => json!({
                "operation": operation,
                "request_id": request(id),
                "task_id": task.to_string(),
                "message": message_value(message),
            }),
            Self::Cancel(id, task) => {
                json!({"operation": operation, "request_id": request(id), "task_id": task.to_string()})
            }
        }
    }
}

fn agent(object: &Map<String, Value>) -> Result<AgentId, ApiError> {
    AgentId::new(text(object, "agent")?).map_err(|_| ApiError::invalid_input())
}

fn task(object: &Map<String, Value>, field: &str) -> Result<AgentTaskId, ApiError> {
    AgentTaskId::parse(text(object, field)?).map_err(|_| ApiError::invalid_input())
}

/// Parses one `agent_*` command.
pub(super) fn parse(object: &Map<String, Value>) -> Result<CommandKind, ApiError> {
    const DEFINE: [&str; 11] = [
        "version",
        "operation",
        "request_id",
        "agent",
        "description",
        "program",
        "args",
        "cwd",
        "env",
        "tools",
        "max_tasks",
    ];
    let named = ["version", "operation", "request_id", "agent"];
    let on_task = ["version", "operation", "request_id", "task_id"];
    let command = match text(object, "operation")? {
        "agent_define" => {
            exact_fields(object, &DEFINE)?;
            AgentCommand::Define(
                request_id(object, "request_id")?,
                Box::new(parse_spec(object)?),
            )
        }
        operation @ ("agent_undefine" | "agent_start" | "agent_stop") => {
            exact_fields(object, &named)?;
            let (id, agent) = (request_id(object, "request_id")?, agent(object)?);
            match operation {
                "agent_undefine" => AgentCommand::Undefine(id, agent),
                "agent_start" => AgentCommand::Start(id, agent),
                _ => AgentCommand::Stop(id, agent),
            }
        }
        "agent_list" => {
            exact_fields(object, &["version", "operation", "after"])?;
            let after = nullable(object, "after")?
                .map(|after| AgentId::new(after).map_err(|_| ApiError::invalid_input()))
                .transpose()?;
            AgentCommand::List(after)
        }
        "agent_task_list" => {
            exact_fields(object, &["version", "operation", "agent", "after"])?;
            let agent = nullable(object, "agent")?
                .map(|agent| AgentId::new(agent).map_err(|_| ApiError::invalid_input()))
                .transpose()?;
            let after = match object.get("after") {
                Some(Value::Null) => None,
                _ => Some(task(object, "after")?),
            };
            AgentCommand::Tasks(agent, after)
        }
        "agent_task_inspect" => {
            exact_fields(object, &["version", "operation", "task_id"])?;
            AgentCommand::Inspect(task(object, "task_id")?)
        }
        "agent_task_events" => {
            exact_fields(object, &["version", "operation", "task_id", "after"])?;
            let after =
                u32::try_from(number(object, "after")?).map_err(|_| ApiError::invalid_input())?;
            AgentCommand::Events(task(object, "task_id")?, after)
        }
        "agent_task_artifact_read" => {
            exact_fields(
                object,
                &["version", "operation", "task_id", "name", "offset"],
            )?;
            let name = text(object, "name")?;
            if name.is_empty() || name.len() > MAX_ARTIFACT_NAME_LEN {
                return Err(ApiError::invalid_input());
            }
            let offset = number(object, "offset")?;
            AgentCommand::Artifact(task(object, "task_id")?, name.to_owned(), offset)
        }
        "agent_task_submit" => {
            exact_fields(
                object,
                &["version", "operation", "request_id", "agent", "message"],
            )?;
            let message = parse_message(object.get("message"))?;
            AgentCommand::Submit(request_id(object, "request_id")?, agent(object)?, message)
        }
        "agent_task_send" => {
            exact_fields(
                object,
                &["version", "operation", "request_id", "task_id", "message"],
            )?;
            let message = parse_message(object.get("message"))?;
            AgentCommand::Send(
                request_id(object, "request_id")?,
                task(object, "task_id")?,
                message,
            )
        }
        "agent_task_cancel" => {
            exact_fields(object, &on_task)?;
            AgentCommand::Cancel(request_id(object, "request_id")?, task(object, "task_id")?)
        }
        _ => return Err(ApiError::invalid_input()),
    };
    Ok(CommandKind::Agent(command))
}
