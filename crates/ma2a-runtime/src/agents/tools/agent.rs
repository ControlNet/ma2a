//! `agent.*`: tasks on other Endpoints' Agents, as this Endpoint.
//!
//! The calling Agent's name travels as `from_agent`, which is attribution only: the target
//! authorizes this Endpoint, never the Agent.

use std::time::Duration;

use ma2a_agent::{
    protocol::ToolError,
    tools::{
        AgentArtifactArgs, AgentSendArgs, AgentSubmitArgs, AgentTaskTarget, AgentWait,
        MAX_AGENT_WAIT_MS, TaskView, Transferred,
    },
};
use ma2a_core::service::{
    AgentId, AgentReply, AgentRequest, AgentSubmit, AgentTaskId, AgentTaskSummary, MessageRole,
    OpenBody, ServiceError,
};
use ma2a_services::client;

use super::{Call, Outcome, bounded, failed, ok, status, workspace::destination};
use crate::agents::convert::{from_wire, hex, task_view};

/// Pause before reaching a target again that was restarting or unreachable.
const RETRY: Duration = Duration::from_secs(1);

/// Returns whether an error only means the target cannot answer right now.
fn transient(error: &ToolError) -> bool {
    [
        ServiceError::Unavailable,
        ServiceError::Unreachable,
        ServiceError::TimedOut,
        ServiceError::OutcomeUnknown,
        ServiceError::Cancelled,
    ]
    .iter()
    .any(|status| status.name() == error.code)
}

fn invalid(what: &str) -> ToolError {
    ToolError::new("invalid_tool", format!("invalid {what}"))
}

fn names(agent: &str, task: &str) -> Result<(AgentId, AgentTaskId), ToolError> {
    Ok((
        AgentId::new(agent).map_err(|_| invalid("Agent name"))?,
        AgentTaskId::parse(task).map_err(|_| invalid("task identifier"))?,
    ))
}

fn summary_view(summary: &AgentTaskSummary) -> TaskView {
    TaskView {
        task: summary.id.to_string(),
        state: summary.state.name().to_owned(),
        reason: summary.reason.map(|reason| reason.name().to_owned()),
        result: None,
        error: None,
        progress: None,
        artifacts: Vec::new(),
    }
}

impl Call<'_> {
    /// Runs one content-free operation and answers its reply, which must be the one the
    /// request asked for: never another task's or another Agent's.
    async fn agent_call(
        &self,
        (endpoint, request): (&str, AgentRequest),
    ) -> Result<AgentReply, ToolError> {
        let body = OpenBody::Agent(request.clone());
        let (mut reader, _writer) = if request.is_mutation() {
            self.open(endpoint, body).await?
        } else {
            self.open_read(endpoint, body).await?
        };
        let (accept, reply) = client::agent::call(&mut reader)
            .await
            .map_err(|error| failed(&error))?;
        if !client::answer::agent(&request, (accept, &reply)) {
            return Err(invalid("Agent answer"));
        }
        Ok(reply)
    }

    async fn summary(&self, (endpoint, request): (&str, AgentRequest)) -> Outcome {
        match self.agent_call((endpoint, request)).await? {
            AgentReply::Summary(summary) => ok(&summary_view(&summary)),
            _ => Err(invalid("Agent answer")),
        }
    }

    async fn view(
        &self,
        endpoint: &str,
        (agent, task): (AgentId, AgentTaskId),
    ) -> Result<TaskView, ToolError> {
        let request = AgentRequest::Get(agent, task);
        let answered = bounded(self.agent_call((endpoint, request))).await?;
        match answered {
            AgentReply::Details(details) => Ok(task_view(&details)),
            _ => Err(invalid("Agent answer")),
        }
    }

    pub(super) async fn agent_submit(&self, args: AgentSubmitArgs) -> Outcome {
        let agent = AgentId::new(&args.agent).map_err(|_| invalid("Agent name"))?;
        let message = from_wire(&args.message).map_err(status)?;
        if message.role() != MessageRole::User {
            return Err(invalid("message role"));
        }
        let submit = AgentSubmit::new(agent, message, Some(self.agent.clone()));
        self.summary((&args.endpoint, AgentRequest::Submit(submit)))
            .await
    }

    pub(super) async fn agent_get(&self, args: AgentTaskTarget) -> Outcome {
        ok(&self
            .view(&args.endpoint, names(&args.agent, &args.task)?)
            .await?)
    }

    pub(super) async fn agent_send(&self, args: AgentSendArgs) -> Outcome {
        let (agent, task) = names(&args.agent, &args.task)?;
        let message = from_wire(&args.message).map_err(status)?;
        self.summary((&args.endpoint, AgentRequest::Send(agent, task, message)))
            .await
    }

    pub(super) async fn agent_cancel(&self, args: AgentTaskTarget) -> Outcome {
        let (agent, task) = names(&args.agent, &args.task)?;
        self.summary((&args.endpoint, AgentRequest::Cancel(agent, task)))
            .await
    }

    /// Waits for the task to end by following its events, then answers its full view.
    pub(super) async fn agent_wait(&self, args: AgentWait) -> Outcome {
        let (agent, task) = names(&args.agent, &args.task)?;
        let timeout = args
            .timeout_ms
            .unwrap_or(MAX_AGENT_WAIT_MS)
            .min(MAX_AGENT_WAIT_MS);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
        let mut after = 0;
        // A view read begun before the deadline may end after it; it is then the last one.
        let retry =
            || tokio::time::sleep_until((tokio::time::Instant::now() + RETRY).min(deadline));
        loop {
            let view = self.view(&args.endpoint, (agent.clone(), task)).await;
            let now = tokio::time::Instant::now();
            let view = match view {
                Ok(view) => view,
                // A target that is restarting or unreachable is waited for, not given up on.
                Err(error) if transient(&error) && now < deadline => {
                    retry().await;
                    continue;
                }
                Err(error) => return Err(error),
            };
            if matches!(view.state.as_str(), "completed" | "failed" | "cancelled")
                || now >= deadline
            {
                return ok(&view);
            }
            let request = AgentRequest::Watch(agent.clone(), task, after);
            let watched = tokio::time::timeout_at(deadline, async {
                let (mut reader, _writer) = self
                    .open_read(&args.endpoint, OpenBody::Agent(request))
                    .await?;
                client::agent::watch(&mut reader, (&agent, task, after), |event| {
                    after = event.seq;
                })
                .await
                .map_err(|error| failed(&error))
            })
            .await;
            match watched {
                Ok(Err(error)) if transient(&error) => retry().await,
                Ok(Err(error)) => return Err(error),
                _ => {}
            }
        }
    }

    pub(super) async fn agent_artifact(&self, args: AgentArtifactArgs) -> Outcome {
        let (agent, task) = names(&args.agent, &args.task)?;
        let _target = destination(&self.workspace, &args.to)?;
        let request = AgentRequest::Artifact(agent, task, args.name, 0, None);
        let body = OpenBody::Agent(request);
        let result = self.download((&args.endpoint, body), &args.to).await?;
        ok(&Transferred {
            size: result.size,
            blake3: hex(&result.hash),
        })
    }
}
