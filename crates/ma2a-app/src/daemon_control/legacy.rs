//! Talking to a daemon that predates the lifecycle plane.
//!
//! Two kinds of daemon can be here without understanding a lifecycle call: one
//! that speaks this local API version but was built before the plane existed,
//! and one that speaks another version entirely. Both still answer the older
//! contract, which is deliberately fixed across versions, and both have to be
//! stoppable — otherwise upgrading the executable would leave a daemon that
//! nothing could replace.

use std::{fmt::Write as _, io};

use ma2a_core::RequestId;
use ma2a_runtime::{
    api,
    ipc::{IpcError, IpcPaths, LocalApiClient},
};

use crate::AppError;

use super::wait_until_released;

/// Asks a same-version daemon without the lifecycle plane to shut down.
///
/// Acceptance is all this establishes. The caller still has to prove that the
/// daemon released the state directory before calling it stopped.
pub(super) async fn request_shutdown(paths: &IpcPaths) -> Result<(), AppError> {
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

/// Stops a daemon of another API version using the version it reports.
pub(super) async fn stop_incompatible(paths: &IpcPaths) -> Result<(), AppError> {
    eprintln!("daemon protocol is incompatible; stopping the existing daemon");
    LocalApiClient::new(paths.clone())
        .shutdown_compatible()
        .await?;
    wait_until_released(paths, None).await
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
