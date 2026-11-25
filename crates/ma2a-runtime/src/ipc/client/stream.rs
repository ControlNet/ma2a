//! Client side of the stream plane: one IPC connection upgraded to carry Services v1 frames.

use std::{
    pin::Pin,
    sync::atomic::Ordering,
    task::{Context, Poll},
};

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};

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
        let (response, stream) = self.upgrade(command, "stream_opened").await?;
        Ok(stream.map_or(StreamUpgrade::Refused(response), StreamUpgrade::Opened))
    }

    /// Upgrades one fresh connection with `service_attach` or `service_accept`, after the
    /// handshake for an attach.
    ///
    /// An accept skips the separate handshake: the daemon serves it from slots kept for
    /// accepts, which a handshake may not use, and the link it answers was already attached
    /// after one. The command's own `version` is still checked.
    ///
    /// Returns the daemon's exact answer, and the upgraded connection when it was accepted:
    /// a link after `service_attached`, or one call's Services v1 frames after
    /// `service_call`.
    ///
    /// # Errors
    /// Returns [`IpcError::InvalidFrame`] for any other command, and a transport, version, or
    /// correlation error.
    pub async fn open_hosted(
        &self,
        command: &Command,
    ) -> Result<(Vec<u8>, Option<LocalStream>), IpcError> {
        match command.operation() {
            "service_attach" => self.upgrade(command, "service_attached").await,
            "service_accept" => self.upgrade_now(command, "service_call").await,
            _ => Err(IpcError::InvalidFrame),
        }
    }

    async fn upgrade(
        &self,
        command: &Command,
        upgraded: &str,
    ) -> Result<(Vec<u8>, Option<LocalStream>), IpcError> {
        validate_handshake(&self.roundtrip(&handshake_command()?).await?)?;
        self.upgrade_now(command, upgraded).await
    }

    /// Upgrades one fresh connection with `command`, without a handshake first.
    async fn upgrade_now(
        &self,
        command: &Command,
        upgraded: &str,
    ) -> Result<(Vec<u8>, Option<LocalStream>), IpcError> {
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
        let opened = value.pointer("/result/type").and_then(Value::as_str) == Some(upgraded);
        Ok((response.payload, opened.then_some(LocalStream(stream))))
    }
}

impl LocalStream {
    /// Splits an attached link into its two directions of local API messages.
    #[must_use]
    pub fn into_link(self) -> (LinkReader, LinkWriter) {
        let (read, write) = tokio::io::split(self.0);
        (LinkReader(read), LinkWriter(write))
    }
}

/// Largest message either side sends on a hosted service's link.
pub const MAX_LINK_MESSAGE_BYTES: usize = 1_024;

/// The Runtime-to-service direction of a link.
#[derive(Debug)]
pub struct LinkReader(ReadHalf<PlatformStream>);

impl LinkReader {
    /// Waits for the next message, as long as the link lasts.
    ///
    /// # Errors
    /// Returns a transport error once the link ended, or a malformed frame.
    pub async fn next(&mut self) -> Result<Vec<u8>, IpcError> {
        Ok(read_reply(&mut self.0, MAX_LINK_MESSAGE_BYTES)
            .await?
            .payload)
    }
}

/// The service-to-Runtime direction of a link.
#[derive(Debug)]
pub struct LinkWriter(WriteHalf<PlatformStream>);

impl LinkWriter {
    /// Sends one message.
    ///
    /// # Errors
    /// Returns a transport error, or an empty or oversized message.
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), IpcError> {
        let frame = FrameRef {
            correlation: 0,
            payload,
            maximum: MAX_LINK_MESSAGE_BYTES,
        };
        write_frame(&mut self.0, frame).await
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
