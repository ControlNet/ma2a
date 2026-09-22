use std::{fmt::Write as _, io};

use ma2a_core::RequestId;
use serde_json::{Value, json};

use super::{IpcError, LocalApiClient, api};
use crate::ipc::{
    LIFECYCLE_DEADLINE,
    lifecycle::{DaemonReport, LifecycleRequest},
};

impl LocalApiClient {
    /// Asks the daemon process to identify itself within a bounded interval.
    ///
    /// # Errors
    /// Returns a transport error, [`IpcError::NoLifecyclePlane`] when the
    /// responder predates this plane, or a timed-out I/O error when no answer
    /// arrives before [`LIFECYCLE_DEADLINE`].
    pub async fn ping(&self) -> Result<DaemonReport, IpcError> {
        self.lifecycle_call(LifecycleRequest::Ping, "pong").await
    }

    /// Asks the daemon process to begin graceful shutdown.
    ///
    /// Acceptance is not teardown: the caller must still prove that the daemon
    /// released its lock before reporting that it stopped.
    ///
    /// # Errors
    /// Returns the same failures as [`Self::ping`].
    pub async fn request_stop(&self) -> Result<DaemonReport, IpcError> {
        self.lifecycle_call(LifecycleRequest::Stop, "stopping")
            .await
    }

    async fn lifecycle_call(
        &self,
        request: LifecycleRequest,
        expected: &str,
    ) -> Result<DaemonReport, IpcError> {
        let response = tokio::time::timeout(
            LIFECYCLE_DEADLINE,
            self.roundtrip_payload(&request.encode()),
        )
        .await
        .map_err(|_elapsed| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "the daemon did not answer a lifecycle call",
            )
        })??;
        let report = DaemonReport::parse(&response).ok_or(IpcError::NoLifecyclePlane)?;
        if report.operation() == expected {
            Ok(report)
        } else {
            Err(IpcError::NoLifecyclePlane)
        }
    }

    /// Requests graceful shutdown using the daemon's reported API version.
    ///
    /// This compatibility exception is only for explicit daemon lifecycle commands.
    /// The framing, version envelope, and graceful shutdown request/acknowledgement
    /// must remain stable across API versions. Business commands still use `call`.
    ///
    /// # Errors
    /// Returns an error if transport fails, the peer does not advertise a valid
    /// version, or it rejects or fails to acknowledge the shutdown request.
    pub async fn shutdown_compatible(&self) -> Result<(), IpcError> {
        let probe = json!({"version": api::LOCAL_API_VERSION, "operation": "handshake"});
        let response = self.roundtrip_payload(&encode(&probe)?).await?;
        let response: Value =
            serde_json::from_slice(&response).map_err(|_| IpcError::VersionMismatch)?;
        let version = response
            .get("version")
            .and_then(Value::as_u64)
            .filter(|version| (1..=u64::from(u16::MAX)).contains(version))
            .ok_or(IpcError::VersionMismatch)?;
        let handshake = response.pointer("/result/type").and_then(Value::as_str)
            == Some("handshake")
            && response.get("error").is_none();
        let mismatch = response.get("error").and_then(Value::as_str) == Some("version_mismatch")
            && response.get("result").is_none();
        if !handshake && !mismatch {
            return Err(IpcError::VersionMismatch);
        }

        let request_id = RequestId::random()
            .map_err(|_| io::Error::other("operating-system random source failed"))?;
        let mut encoded_id = String::with_capacity(32);
        for byte in request_id.as_bytes() {
            write!(&mut encoded_id, "{byte:02x}")
                .map_err(|_| io::Error::other("request identifier encoding failed"))?;
        }
        let request = json!({
            "version": version,
            "operation": "graceful_shutdown",
            "request_id": encoded_id,
        });
        let response = self.roundtrip_payload(&encode(&request)?).await?;
        let response: Value =
            serde_json::from_slice(&response).map_err(|_| IpcError::InvalidFrame)?;
        if response.get("version").and_then(Value::as_u64) != Some(version)
            || response.get("request_id").and_then(Value::as_str) != Some(encoded_id.as_str())
            || response.get("error").is_some()
            || response.pointer("/result/type").and_then(Value::as_str) != Some("shutting_down")
        {
            return Err(io::Error::other("daemon rejected the compatible stop request").into());
        }
        Ok(())
    }
}

fn encode(value: &Value) -> Result<Vec<u8>, IpcError> {
    serde_json::to_vec(value).map_err(|error| io::Error::other(error).into())
}
