//! Opening one remote operation through this Runtime's own local API, exactly as the CLI
//! does: the daemon dials the target as itself and relays the Services v1 frames.

use std::time::Duration;

use ma2a_agent::protocol::ToolError;
use ma2a_core::{
    RequestId, ServiceKind,
    service::{OpenBody, OpenHeader, ServiceError},
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

/// Longest wait for this Runtime's local API server to be bound.
const LOOPBACK_WAIT: Duration = Duration::from_secs(30);

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

    /// Opens one operation as its kind needs: a mutation under the call's derived
    /// `RequestId`, a read under a fresh one; returns the `RequestId` it was opened under.
    pub(super) async fn open_bound(
        &self,
        endpoint: &str,
        body: OpenBody,
    ) -> Result<(RequestId, Halves), ToolError> {
        let request_id = if body.is_mutation() {
            self.request_id
        } else {
            RequestId::random().map_err(|_| status(ServiceError::Internal))?
        };
        Ok((
            request_id,
            self.open_as((endpoint, request_id), body).await?,
        ))
    }

    async fn open_as(
        &self,
        (endpoint, request_id): (&str, RequestId),
        body: OpenBody,
    ) -> Result<Halves, ToolError> {
        let paths = self
            .loopback()
            .await
            .ok_or_else(|| status(ServiceError::Unavailable))?;
        let command = stream_open((endpoint, request_id), body.kind())?;
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
        write_open_header(&mut writer, &header).await?;
        Ok((reader, writer))
    }
}

/// Writes and flushes the initial `open` frame of one remote tool call.
async fn write_open_header<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    header: &[u8],
) -> Result<(), ToolError> {
    writer.write_open(header).await.map_err(status)
}

impl Call<'_> {
    /// Checks a target Endpoint as opening an operation on it would: an invalid one is
    /// `invalid_tool`, and this Runtime itself is `invalid_input`, as the local API answers.
    pub(super) fn check_target(&self, endpoint: &str, kind: ServiceKind) -> Result<(), ToolError> {
        let request_id = RequestId::random().map_err(|_| status(ServiceError::Internal))?;
        let command = stream_open((endpoint, request_id), kind)?;
        match command.stream_open() {
            Some((_, target, _)) if target == self.manager.local() => {
                Err(status(ServiceError::InvalidInput))
            }
            Some(_) => Ok(()),
            None => Err(status(ServiceError::Internal)),
        }
    }

    /// Returns this Runtime's own local API paths once its server is bound: a call
    /// re-run during recovery may start before it is.
    async fn loopback(&self) -> Option<crate::ipc::IpcPaths> {
        let deadline = tokio::time::Instant::now() + LOOPBACK_WAIT;
        loop {
            if let Some(paths) = self.manager.0.loopback.get() {
                return Some(paths.clone());
            }
            if tokio::time::Instant::now() >= deadline || self.manager.0.shutdown.is_cancelled() {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Builds the local API `stream_open` of one operation; an invalid target is `invalid_tool`.
fn stream_open(
    (endpoint, request_id): (&str, RequestId),
    kind: ServiceKind,
) -> Result<api::Command, ToolError> {
    let request = json!({
        "version": 1,
        "operation": "stream_open",
        "request_id": hex(request_id.as_bytes()),
        "target_endpoint_id": endpoint,
        "service": kind.name(),
    });
    let encoded = serde_json::to_vec(&request).map_err(|_| status(ServiceError::Internal))?;
    api::decode_command(&encoded)
        .map_err(|_| ToolError::new("invalid_tool", "invalid target Endpoint"))
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

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use ma2a_services::FrameWriter;

    use super::write_open_header;

    struct FailsAfterWrite {
        remaining: usize,
        fail_flush: bool,
    }

    impl tokio::io::AsyncWrite for FailsAfterWrite {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            if self.remaining == 0 {
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            let take = buffer.len().min(self.remaining);
            self.remaining -= take;
            Poll::Ready(Ok(take))
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            if self.fail_flush {
                Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
            } else {
                Poll::Ready(Ok(()))
            }
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn broken_open_write_after_sent_bytes_reports_outcome_unknown() {
        let mut before_any_byte = FrameWriter::new(FailsAfterWrite {
            remaining: 0,
            fail_flush: false,
        });
        let early = write_open_header(&mut before_any_byte, b"open-header")
            .await
            .expect_err("write before any byte fails");
        assert_eq!(early.code, "cancelled");

        for (remaining, fail_flush) in [(2, false), (5, false), (7, false), (usize::MAX, true)] {
            let mut writer = FrameWriter::new(FailsAfterWrite {
                remaining,
                fail_flush,
            });
            let error = write_open_header(&mut writer, b"open-header")
                .await
                .expect_err("partial write or failed flush fails");
            assert_eq!(
                error.code, "outcome_unknown",
                "remaining={remaining}, fail_flush={fail_flush}"
            );
        }
    }
}
