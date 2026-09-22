use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use crate::api::{self, Command};

use super::{
    COMMAND_DEADLINE, IpcError, IpcPaths,
    framing::{FrameRef, read_frame_within, write_frame},
    platform, response_deadline,
};

static NEXT_CORRELATION: AtomicU64 = AtomicU64::new(1);

mod lifecycle;

#[cfg(all(test, unix))]
#[path = "client_deadline_tests.rs"]
mod deadline_tests;

/// Exact-version client for the current user's local daemon.
#[derive(Clone, Debug)]
pub struct LocalApiClient {
    paths: IpcPaths,
}

impl LocalApiClient {
    /// Creates a client for the supplied private transport paths.
    pub const fn new(paths: IpcPaths) -> Self {
        Self { paths }
    }

    /// Performs a handshake when required and sends one typed command.
    ///
    /// # Errors
    /// Returns a transport, version, correlation, or local API error.
    pub async fn call(&self, command: &Command) -> Result<Vec<u8>, IpcError> {
        if command.operation() != "handshake" {
            let handshake = handshake_command()?;
            let response = self.roundtrip(&handshake).await?;
            validate_handshake(&response)?;
        }
        self.roundtrip(command).await
    }

    /// Reports whether an exact-version daemon answers the private handshake.
    pub async fn is_live(&self) -> bool {
        self.probe().await.is_ok()
    }

    /// Verifies that an exact-version daemon answers the private handshake.
    ///
    /// # Errors
    /// Returns a transport error or [`IpcError::VersionMismatch`].
    pub async fn probe(&self) -> Result<(), IpcError> {
        let command = handshake_command();
        match command {
            Ok(command) => self
                .roundtrip(&command)
                .await
                .and_then(|response| validate_handshake(&response)),
            Err(error) => Err(error),
        }
    }

    async fn roundtrip(&self, command: &Command) -> Result<Vec<u8>, IpcError> {
        let payload = api::encode_command(command)?;
        // A command that legitimately performs bounded remote work must be given
        // its own completion deadline; the transport deadline still bounds the
        // frame itself once the Runtime starts answering.
        self.roundtrip_within(&payload, response_deadline(command.operation()))
            .await
    }

    async fn roundtrip_payload(&self, payload: &[u8]) -> Result<Vec<u8>, IpcError> {
        self.roundtrip_within(payload, COMMAND_DEADLINE).await
    }

    async fn roundtrip_within(
        &self,
        payload: &[u8],
        deadline: std::time::Duration,
    ) -> Result<Vec<u8>, IpcError> {
        let correlation = NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed);
        let mut stream = platform::connect(&self.paths).await?;
        write_frame(
            &mut stream,
            FrameRef {
                correlation,
                payload,
                maximum: api::MAX_LOCAL_REQUEST_BYTES,
            },
        )
        .await?;
        let response =
            read_frame_within(&mut stream, api::MAX_LOCAL_RESPONSE_BYTES, deadline).await?;
        if response.correlation != correlation {
            return Err(IpcError::CorrelationMismatch);
        }
        Ok(response.payload)
    }
}

fn handshake_command() -> Result<Command, IpcError> {
    let request = format!(
        r#"{{"version":{},"operation":"handshake"}}"#,
        api::LOCAL_API_VERSION
    );
    api::decode_command(request.as_bytes()).map_err(Into::into)
}

fn validate_handshake(response: &[u8]) -> Result<(), IpcError> {
    let value: Value = serde_json::from_slice(response).map_err(|_| IpcError::VersionMismatch)?;
    let version = value.get("version").and_then(Value::as_u64);
    let result_type = value.pointer("/result/type").and_then(Value::as_str);
    if version == Some(u64::from(api::LOCAL_API_VERSION)) && result_type == Some("handshake") {
        Ok(())
    } else {
        Err(IpcError::VersionMismatch)
    }
}
