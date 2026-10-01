//! The tool bridge: an Agent's tool call runs as a same-user stream-plane client of this
//! Runtime, with exactly the hosting Endpoint's authority, replay, and audit.
//!
//! A call is recorded before it runs and its result before it is delivered, so a pending
//! call of a live task is executed again after a restart under the same derived
//! `RequestId`, which the stream plane's durable replay answers without a second effect.

mod agent;
mod exec;
mod files;
mod proxy;
mod remote;
mod workspace;

use std::path::PathBuf;

use ma2a_agent::{
    protocol::{MAX_TOOL_RESULT_LEN, ToolError},
    tools::{Tool, ToolFamily},
};
use ma2a_core::{
    RequestId,
    service::{AgentId, AgentTaskId, ServiceError},
};
use ma2a_services::client::ClientError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use super::{AgentManager, fault};

/// The answer to one call.
type Outcome = Result<Value, ToolError>;

/// Everything one executing call needs.
#[derive(Debug)]
struct Call<'a> {
    manager: &'a AgentManager,
    task: AgentTaskId,
    agent: AgentId,
    workspace: PathBuf,
    request_id: RequestId,
}

impl Call<'_> {
    /// Parses the call's arguments into one tool's typed shape.
    fn args<T: DeserializeOwned>(args: Value) -> Result<T, ToolError> {
        serde_json::from_value(args)
            .map_err(|error| ToolError::new("invalid_tool", error.to_string()))
    }
}

/// Returns a tool's typed result as the call's answer.
fn ok(result: &impl Serialize) -> Outcome {
    serde_json::to_value(result).map_err(|_| status(ServiceError::Internal))
}

/// Returns the error of one Services v1 status.
fn status(error: ServiceError) -> ToolError {
    ToolError::new(error.name(), error.to_string())
}

/// Returns the error of one failed caller-side operation.
fn failed(error: &ClientError) -> ToolError {
    error.status().map_or_else(
        || ToolError::new("local_io", error.to_string()),
        |code| ToolError::new(code.name(), error.to_string()),
    )
}

/// Encodes one answer as it is stored: `{"ok": …}` or `{"error": {…}}`.
fn stored(outcome: &Outcome) -> Vec<u8> {
    let value = match outcome {
        Ok(result) => json!({ "ok": result }),
        Err(error) => json!({ "error": error }),
    };
    serde_json::to_vec(&value).unwrap_or_default()
}

/// Encodes one answer within [`MAX_TOOL_RESULT_LEN`]; a larger one is `too_large`.
fn answer(outcome: &Outcome) -> Vec<u8> {
    let bytes = stored(outcome);
    if bytes.len() <= MAX_TOOL_RESULT_LEN {
        return bytes;
    }
    stored(&Err(ToolError::new(
        "too_large",
        "the tool result exceeds 64 KiB",
    )))
}

/// The stored answer of a call refused because its task has too many open.
pub(super) fn rejection() -> Vec<u8> {
    stored(&Err(status(ServiceError::ConcurrencyExceeded)))
}

/// Derives the `RequestId` of one call from its task and call identifier.
fn call_request(task: AgentTaskId, call: &str) -> Result<RequestId, ServiceError> {
    let mut hasher = blake3::Hasher::new_derive_key("ma2a agent tool call v1");
    hasher.update(task.as_bytes());
    hasher.update(call.as_bytes());
    RequestId::try_from(hasher.finalize().as_bytes().get(..16).unwrap_or_default())
        .map_err(|_| ServiceError::Internal)
}

impl AgentManager {
    /// Runs one recorded call unless it already runs, then records and delivers its answer.
    pub(super) fn run_tool(&self, (task, call): (AgentTaskId, String)) {
        if !self.coordination().calls.insert((task, call.clone())) {
            return;
        }
        let manager = self.clone();
        self.0.tasks.spawn(async move {
            if let Some(outcome) = manager.execute((task, &call)).await {
                fault("before_tool_result");
                let (bytes, at_ms) = (answer(&outcome), manager.now_ms());
                let id = call.clone();
                let _recorded = manager
                    .commit(move |repository| {
                        repository.complete_tool_call((task, &id), (&bytes, at_ms))
                    })
                    .await;
            }
            manager.coordination().calls.remove(&(task, call));
        });
    }

    /// Executes one pending call; `None` when it is not pending any more.
    async fn execute(&self, (task, call): (AgentTaskId, &str)) -> Option<Outcome> {
        let id = call.to_owned();
        let recorded = self
            .store(move |repository| repository.tool_call((task, &id)))
            .await
            .ok()
            .flatten()
            .filter(|recorded| recorded.result.is_none())?;
        fault("after_tool_recorded");
        let record = self.task(task).await.ok().flatten()?;
        if record.summary.state.is_terminal() {
            return Some(Err(status(ServiceError::TaskEnded)));
        }
        let agent = record.summary.agent.clone();
        let definition = self.agent(&agent).await.ok().flatten()?.definition;
        let Some(tool) = Tool::from_name(&recorded.name) else {
            return Some(Err(ToolError::new("invalid_tool", "no such tool")));
        };
        if tool
            .family()
            .is_some_and(|family: ToolFamily| definition.tools & family.bit() == 0)
        {
            return Some(Err(ToolError::new(
                "tool_denied",
                "the Agent's definition does not allow this tool family",
            )));
        }
        let Ok(args) = serde_json::from_slice::<Value>(&recorded.request) else {
            return Some(Err(ToolError::new(
                "invalid_tool",
                "arguments are not JSON",
            )));
        };
        let context = Call {
            manager: self,
            task,
            workspace: self.workspace(&agent),
            agent,
            request_id: call_request(task, call).ok()?,
        };
        Some(context.run(tool, args).await)
    }
}

impl Call<'_> {
    async fn run(&self, tool: Tool, args: Value) -> Outcome {
        match tool {
            Tool::FilesStat => self.files_stat(Self::args(args)?).await,
            Tool::FilesList => self.files_list(Self::args(args)?).await,
            Tool::TransferGet => self.transfer_get(Self::args(args)?).await,
            Tool::TransferPut => self.transfer_put(Self::args(args)?).await,
            Tool::ArtifactPublish => self.artifact_publish(Self::args(args)?).await,
            Tool::ExecRun => self.exec_run(Self::args(args)?).await,
            Tool::ProcessStart => self.process_start(Self::args(args)?).await,
            Tool::ProcessInspect => self.process_inspect(Self::args(args)?).await,
            Tool::ProcessTerminate => self.process_terminate(Self::args(args)?).await,
            Tool::ProxyOpen => self.proxy_open(Self::args(args)?).await,
            Tool::AgentSubmit => self.agent_submit(Self::args(args)?).await,
            Tool::AgentGet => self.agent_get(Self::args(args)?).await,
            Tool::AgentWait => self.agent_wait(Self::args(args)?).await,
            Tool::AgentSend => self.agent_send(Self::args(args)?).await,
            Tool::AgentCancel => self.agent_cancel(Self::args(args)?).await,
            Tool::AgentArtifact => self.agent_artifact(Self::args(args)?).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use ma2a_agent::protocol::MAX_TOOL_RESULT_LEN;
    use serde_json::json;

    use super::answer;

    #[test]
    fn an_answer_over_the_stored_bound_becomes_too_large() {
        let small = answer(&Ok(json!({"size": 3})));
        assert_eq!(small, br#"{"ok":{"size":3}}"#);
        let huge = answer(&Ok(json!({"text": "x".repeat(MAX_TOOL_RESULT_LEN)})));
        assert!(huge.len() <= MAX_TOOL_RESULT_LEN);
        let decoded: serde_json::Value = serde_json::from_slice(&huge).unwrap_or_default();
        assert_eq!(decoded.pointer("/error/code"), Some(&json!("too_large")));
    }
}
