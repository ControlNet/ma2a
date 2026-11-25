//! Following a remote task with `watch` to its end, across interruptions of the target.

use std::time::Duration;

use ma2a_core::service::{AgentId, AgentReply, AgentRequest, AgentTaskId, OpenBody, ServiceError};
use ma2a_runtime::{api::agent_event_value, ipc::IpcError};
use ma2a_services::client;

use super::{
    agent::{Target, print},
    open, own_endpoint,
};
use crate::AppError;

/// Pause before reaching a target again that is restarting or unreachable.
const RETRY: Duration = Duration::from_secs(1);

/// Returns whether the status `name` only means the target cannot answer right now: it is
/// restarting or out of reach.
pub(crate) fn passes(name: &str) -> bool {
    [
        ServiceError::Unavailable,
        ServiceError::Unreachable,
        ServiceError::TimedOut,
        ServiceError::OutcomeUnknown,
        ServiceError::Cancelled,
    ]
    .iter()
    .any(|status| status.name() == name)
}

/// Returns whether reopening an interrupted watch failed in a way that passes: the Runtime
/// keeps the meaning of its own refusals, a Runtime of another local API version or an IPC
/// path that is not this user's stays that way, and any other local failure is this Runtime
/// being away for now, as while it restarts.
pub(crate) fn reopening_passes(error: &AppError) -> bool {
    match error {
        AppError::Rejected(name, _) => passes(name),
        AppError::Ipc(
            IpcError::VersionMismatch | IpcError::UnauthorizedPeer | IpcError::InvalidPath,
        )
        | AppError::DaemonConflicted(_) => false,
        AppError::Io(error) | AppError::Ipc(IpcError::Io(error))
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            false
        }
        _ => true,
    }
}

/// Follows a task until it ended, printing its events after `after` when given, and
/// reconnecting while the target restarts or is unreachable. Once a watch was interrupted,
/// reopening it can fail the same passing way, so that is retried too.
pub(super) async fn follow(
    target: Target<'_>,
    (agent, task): (AgentId, AgentTaskId),
    after: Option<u32>,
) -> Result<AgentReply, AppError> {
    let (print_events, mut seen) = (after.is_some(), after.unwrap_or(0));
    // The task's end once taken, which a reconnected watch must end with and nothing follow.
    let mut ended = None;
    let caller = own_endpoint(target.0).await?;
    let mut reconnecting = false;
    loop {
        let request = AgentRequest::Watch(agent.clone(), task, seen);
        let watched = match open(target.0, target.1, OpenBody::Agent(request)).await {
            Ok((mut reader, _writer)) => {
                let mut printed = Ok(());
                let outcome = client::agent::watch(
                    &mut reader,
                    ((&caller, &agent, task), (seen, ended)),
                    |event| {
                        (seen, ended) = (event.seq, client::agent::end_of(&event));
                        if print_events && printed.is_ok() {
                            printed = print(&agent_event_value(&event));
                        }
                    },
                )
                .await;
                printed?;
                outcome
            }
            Err(error) if reconnecting && reopening_passes(&error) => {
                tokio::time::sleep(RETRY).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        match watched {
            Ok((_, reply)) => return Ok(reply),
            Err(error) if error.status().is_some_and(|status| passes(status.name())) => {
                reconnecting = true;
                tokio::time::sleep(RETRY).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
