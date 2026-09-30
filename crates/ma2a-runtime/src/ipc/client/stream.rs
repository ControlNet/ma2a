//! Client side of the stream plane: one IPC connection upgraded to carry Services v1 frames.

use std::{
    pin::Pin,
    sync::atomic::Ordering,
    task::{Context, Poll},
};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{
    super::{
        IpcError,
        framing::{FrameRef, read_reply, write_frame},
        platform::{self, PlatformStream},
    },
    LocalApiClient, NEXT_CORRELATION, handshake_command, validate_handshake,
};
use crate::api::{self, Command};

/// An upgraded connection: Services v1 frames flow verbatim in both directions.
///
/// Keep the write side open until a terminal frame arrives; closing it earlier cancels the
/// operation on the target.
#[derive(Debug)]
pub struct LocalStream(PlatformStream);

/// The daemon's answer to one `stream_open`.
#[derive(Debug)]
#[non_exhaustive]
pub enum StreamUpgrade {
    /// The connection now carries frames.
    Opened(LocalStream),
    /// The daemon refused with this exact local API error response.
    Refused(Vec<u8>),
}

impl LocalApiClient {
    /// Performs the handshake, then upgrades one fresh connection with `stream_open`.
    ///
    /// # Errors
    /// Returns [`IpcError::InvalidFrame`] for any command other than `stream_open`, and a
    /// transport, version, or correlation error.
    pub async fn open_stream(&self, command: &Command) -> Result<StreamUpgrade, IpcError> {
        if command.stream_open().is_none() {
            return Err(IpcError::InvalidFrame);
        }
        validate_handshake(&self.roundtrip(&handshake_command()?).await?)?;
        let correlation = NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed);
        let mut stream = platform::connect(&self.paths).await?;
        write_frame(
            &mut stream,
            FrameRef {
                correlation,
                payload: &api::encode_command(command)?,
                maximum: api::MAX_LOCAL_REQUEST_BYTES,
            },
        )
        .await?;
        let response = read_reply(&mut stream, api::MAX_LOCAL_RESPONSE_BYTES).await?;
        if response.correlation != correlation {
            return Err(IpcError::CorrelationMismatch);
        }
        let value: Value =
            serde_json::from_slice(&response.payload).map_err(|_| IpcError::InvalidFrame)?;
        Ok(
            if value.pointer("/result/type").and_then(Value::as_str) == Some("stream_opened") {
                StreamUpgrade::Opened(LocalStream(stream))
            } else {
                StreamUpgrade::Refused(response.payload)
            },
        )
    }
}

impl AsyncRead for LocalStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_read(context, buffer)
    }
}

impl AsyncWrite for LocalStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(context, bytes)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(context)
    }
}
