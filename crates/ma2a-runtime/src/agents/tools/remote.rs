//! Opening one remote operation through this Runtime's own local API, exactly as the CLI
//! does: the daemon dials the target as itself and relays the Services v1 frames.

use ma2a_agent::protocol::ToolError;
use ma2a_core::{
    RequestId,
    service::{FrameKind, OpenBody, OpenHeader, ServiceError},
};
use ma2a_services::{FrameReader, FrameWriter};
use serde_json::{Value, json};
use tokio::io::{ReadHalf, WriteHalf};

use super::{Call, status};
use crate::{
    agents::convert::hex,
    api,
    ipc::{LocalApiClient, LocalStream, StreamUpgrade},
};

/// The caller's frame halves of one opened operation.
pub(super) type Halves = (
    FrameReader<ReadHalf<LocalStream>>,
    FrameWriter<WriteHalf<LocalStream>>,
);

impl Call<'_> {
    /// Opens one remote operation under the call's derived `RequestId`.
    pub(super) async fn open(&self, endpoint: &str, body: OpenBody) -> Result<Halves, ToolError> {
        self.open_as((endpoint, self.request_id), body).await
    }

    /// Opens one read-only remote operation under a fresh `RequestId`.
    pub(super) async fn open_read(
        &self,
        endpoint: &str,
        body: OpenBody,
    ) -> Result<Halves, ToolError> {
        let request_id = RequestId::random().map_err(|_| status(ServiceError::Internal))?;
        self.open_as((endpoint, request_id), body).await
    }

    async fn open_as(
        &self,
        (endpoint, request_id): (&str, RequestId),
        body: OpenBody,
    ) -> Result<Halves, ToolError> {
        let paths = self
            .manager
            .0
            .loopback
            .get()
            .ok_or_else(|| status(ServiceError::Unavailable))?;
        let request = json!({
            "version": 1,
            "operation": "stream_open",
            "request_id": hex(request_id.as_bytes()),
            "target_endpoint_id": endpoint,
            "service": body.kind().name(),
        });
        let encoded = serde_json::to_vec(&request).map_err(|_| status(ServiceError::Internal))?;
        let command = api::decode_command(&encoded)
            .map_err(|_| ToolError::new("invalid_tool", "invalid target Endpoint"))?;
        let (_, target, _) = command
            .stream_open()
            .ok_or_else(|| status(ServiceError::Internal))?;
        let upgrade = LocalApiClient::new(paths.clone())
            .open_stream(&command)
            .await
            .map_err(|_| status(ServiceError::Unavailable))?;
        let stream = match upgrade {
            StreamUpgrade::Opened(stream) => stream,
            StreamUpgrade::Refused(response) => return Err(refused(&response)),
        };
        let (read, write) = tokio::io::split(stream);
        let (reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
        let header = OpenHeader::new(request_id, target, body)
            .encode()
            .map_err(|_| ToolError::new("invalid_tool", "the request exceeds a protocol bound"))?;
        writer
            .frame(FrameKind::Open, &header)
            .await
            .map_err(status)?;
        writer.flush().await.map_err(status)?;
        Ok((reader, writer))
    }
}

/// Turns a local API refusal into the call's error.
fn refused(response: &[u8]) -> ToolError {
    let document: Value = serde_json::from_slice(response).unwrap_or(Value::Null);
    let code = document
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("unavailable");
    let message = document
        .get("remediation")
        .and_then(Value::as_str)
        .unwrap_or("this Runtime refused the stream");
    ToolError::new(code, message)
}
