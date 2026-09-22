//! Talking to a daemon that predates the lifecycle plane.
//!
//! Such a daemon can only be asked to stop through `graceful_shutdown`, which
//! runs on the business plane: it takes the durable replay lock, reaches the
//! Runtime actor and commits a record. That is exactly the plane the lifecycle
//! plane exists to avoid, so a Runtime that will never answer would otherwise
//! make `stop` and `restart` wait for good. Every operation here is therefore
//! wrapped in one explicit compatibility deadline. The old global two-second
//! business timeout is not coming back: it is only this path, and only because
//! this path has no other way to be bounded.

use std::{fmt::Write as _, io, time::Duration};

use ma2a_core::RequestId;
use ma2a_runtime::{
    api,
    ipc::{IpcError, IpcPaths, LocalApiClient},
};

use crate::AppError;

use super::wait_until_released;

/// Bounds one complete compatibility shutdown, request and acknowledgement.
///
/// Every daemon that needs this path was built alongside clients that gave the
/// whole exchange two seconds, so none of them can depend on being given more.
/// This allows several times that, because the command legitimately commits
/// durable state, and it is finite because a lifecycle command must return.
const COMPATIBILITY_DEADLINE: Duration = Duration::from_secs(10);

/// Tells the operator that the daemon in place is being replaced, not reused.
pub(super) fn announce_replacement() {
    eprintln!("daemon protocol is incompatible; stopping the existing daemon");
}

/// Stops a same-version daemon that has no lifecycle plane.
pub(super) async fn stop_current_api(paths: &IpcPaths) -> Result<(), AppError> {
    bounded(request_shutdown(paths)).await?;
    wait_until_released(paths, None).await
}

/// Stops a daemon of another API version using the version it reports.
pub(super) async fn stop_incompatible(paths: &IpcPaths) -> Result<(), AppError> {
    announce_replacement();
    bounded(LocalApiClient::new(paths.clone()).shutdown_compatible()).await?;
    wait_until_released(paths, None).await
}

/// Runs one compatibility exchange, or reports that it never finished.
async fn bounded<E: Into<AppError>>(
    exchange: impl Future<Output = Result<(), E>>,
) -> Result<(), AppError> {
    match tokio::time::timeout(COMPATIBILITY_DEADLINE, exchange).await {
        Ok(result) => result.map_err(Into::into),
        Err(_elapsed) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the daemon did not answer the compatible stop request",
        )
        .into()),
    }
}

/// Asks a same-version daemon without the lifecycle plane to shut down.
///
/// Acceptance is all this establishes. The caller still has to prove that the
/// daemon released the state directory before calling it stopped.
async fn request_shutdown(paths: &IpcPaths) -> Result<(), AppError> {
    let command = shutdown_command()?;
    let response = LocalApiClient::new(paths.clone()).call(&command).await?;
    let document: serde_json::Value =
        serde_json::from_slice(&response).map_err(io::Error::other)?;
    if document
        .pointer("/result/type")
        .and_then(serde_json::Value::as_str)
        != Some("shutting_down")
    {
        return Err(io::Error::other("daemon rejected the stop request").into());
    }
    Ok(())
}

fn shutdown_command() -> Result<api::Command, AppError> {
    let request_id = RequestId::random()
        .map_err(|_| io::Error::other("operating-system random source failed"))?;
    let mut encoded_id = String::with_capacity(32);
    for byte in request_id.as_bytes() {
        write!(&mut encoded_id, "{byte:02x}")
            .map_err(|_| io::Error::other("request identifier encoding failed"))?;
    }
    let request = format!(
        r#"{{"version":{},"operation":"graceful_shutdown","request_id":"{encoded_id}"}}"#,
        api::LOCAL_API_VERSION
    );
    Ok(api::decode_command(request.as_bytes()).map_err(IpcError::from)?)
}

#[cfg(test)]
mod tests {
    use super::shutdown_command;

    #[test]
    fn shutdown_commands_use_fresh_request_identifiers() {
        // Given
        let first = shutdown_command().expect("first shutdown command");

        // When
        let second = shutdown_command().expect("second shutdown command");

        // Then
        assert_ne!(first.request_id(), second.request_id());
    }
}
