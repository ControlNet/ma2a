use std::{fmt::Write as _, io};

use ma2a_core::RequestId;
use serde_json::{Value, json};

use super::{IpcError, LocalApiClient, api};

impl LocalApiClient {
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
