//! Bounded asynchronous service frame reading and writing.

use ma2a_core::service::{
    Channel, FRAME_HEADER_LEN, FrameHeader, FrameKind, MAX_DATA_CHUNK_LEN, ServiceError,
};
use tokio::io::{AsyncRead, AsyncReadExt as _, AsyncWrite, AsyncWriteExt as _};

/// One received frame whose length was bounded before its payload was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    kind: FrameKind,
    payload: Vec<u8>,
}

impl Frame {
    /// Returns the frame type.
    pub const fn kind(&self) -> FrameKind {
        self.kind
    }

    /// Returns the whole payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Returns the channel and bytes of a `data` frame.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for another frame type or an unknown channel.
    pub fn data(&self) -> Result<(Channel, &[u8]), ServiceError> {
        match (self.kind, self.payload.split_first()) {
            (FrameKind::Data, Some((&channel, bytes))) => Ok((channel_from(channel)?, bytes)),
            _ => Err(ServiceError::InvalidInput),
        }
    }

    /// Returns the channel of an `end` frame.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for another frame type or an unknown channel.
    pub const fn end(&self) -> Result<Channel, ServiceError> {
        match (self.kind, self.payload.as_slice()) {
            (FrameKind::End, [channel]) => channel_from(*channel),
            _ => Err(ServiceError::InvalidInput),
        }
    }

    /// Returns the status carried by a `reject` frame.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for another frame type or an unknown status.
    pub fn rejection(&self) -> Result<ServiceError, ServiceError> {
        match (self.kind, self.payload.as_slice()) {
            (FrameKind::Reject, [code]) => {
                ServiceError::from_code(*code).ok_or(ServiceError::InvalidInput)
            }
            _ => Err(ServiceError::InvalidInput),
        }
    }

    /// Consumes the frame into its payload.
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }
}

const fn channel_from(code: u8) -> Result<Channel, ServiceError> {
    match code {
        0 => Ok(Channel::Input),
        1 => Ok(Channel::Output),
        2 => Ok(Channel::Error),
        _ => Err(ServiceError::InvalidInput),
    }
}

/// Reads frames from one stream half, never buffering more than one bounded frame.
///
/// Reading is cancel-safe: a partially received frame is kept here, so a `next` dropped in
/// a `select!` resumes exactly where it stopped and no byte is ever lost.
#[derive(Debug)]
pub struct FrameReader<R> {
    inner: R,
    header: [u8; FRAME_HEADER_LEN],
    header_filled: usize,
    /// The frame whose header is complete, and how much of its payload has arrived.
    partial: Option<(FrameKind, Vec<u8>, usize)>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Wraps one readable half.
    pub const fn new(inner: R) -> Self {
        Self {
            inner,
            header: [0; FRAME_HEADER_LEN],
            header_filled: 0,
            partial: None,
        }
    }

    /// Reads the next frame, or `None` at a clean end of stream between frames.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for an unknown type or out-of-bound length, and
    /// [`ServiceError::Cancelled`] when the stream ends or resets inside a frame.
    pub async fn next(&mut self) -> Result<Option<Frame>, ServiceError> {
        while self.partial.is_none() {
            let rest = self
                .header
                .get_mut(self.header_filled..)
                .ok_or(ServiceError::InvalidInput)?;
            let read = self
                .inner
                .read(rest)
                .await
                .map_err(|_| ServiceError::Cancelled)?;
            match (read, self.header_filled) {
                (0, 0) => return Ok(None),
                (0, _) => return Err(ServiceError::Cancelled),
                _ => self.header_filled += read,
            }
            if self.header_filled == FRAME_HEADER_LEN {
                self.header_filled = 0;
                let header =
                    FrameHeader::parse(self.header).map_err(|_| ServiceError::InvalidInput)?;
                self.partial = Some((header.kind(), vec![0_u8; header.length()], 0));
            }
        }
        let Some((kind, payload, filled)) = self.partial.as_mut() else {
            return Err(ServiceError::Internal);
        };
        while *filled < payload.len() {
            let rest = payload.get_mut(*filled..).ok_or(ServiceError::Internal)?;
            let read = self
                .inner
                .read(rest)
                .await
                .map_err(|_| ServiceError::Cancelled)?;
            if read == 0 {
                return Err(ServiceError::Cancelled);
            }
            *filled += read;
        }
        let kind = *kind;
        let payload = std::mem::take(payload);
        self.partial = None;
        Ok(Some(Frame { kind, payload }))
    }

    /// Reads the next frame and requires it to exist.
    ///
    /// # Errors
    /// Returns [`ServiceError::Cancelled`] at end of stream, plus every [`Self::next`] error.
    pub async fn expect(&mut self) -> Result<Frame, ServiceError> {
        self.next().await?.ok_or(ServiceError::Cancelled)
    }

    /// Returns the wrapped half.
    pub fn into_inner(self) -> R {
        self.inner
    }
}

/// Writes frames to one stream half without concatenating payloads.
///
/// A frame whose write was abandoned part-way, as when a deadline drops the write, leaves the
/// writer torn: the peer still expects the rest of that frame, so no later frame is written.
#[derive(Debug)]
pub struct FrameWriter<W> {
    inner: W,
    torn: bool,
    terminal_sent: bool,
    wrote: bool,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    /// Wraps one writable half.
    pub const fn new(inner: W) -> Self {
        Self {
            inner,
            torn: false,
            terminal_sent: false,
            wrote: false,
        }
    }

    /// Returns whether a frame was abandoned part-way, so the stream can only be reset.
    pub const fn is_torn(&self) -> bool {
        self.torn
    }

    /// Returns whether a complete terminal `outcome` or `reject` frame was written.
    pub const fn terminal_sent(&self) -> bool {
        self.terminal_sent
    }

    /// Returns whether any byte has been handed to the underlying writer.
    pub const fn has_written(&self) -> bool {
        self.wrote
    }

    /// Writes and flushes the initial `open` frame of a caller stream.
    ///
    /// Keeps [`ServiceError::Cancelled`] only when no byte reached the underlying writer; once
    /// any byte was written, a write or flush failure becomes [`ServiceError::OutcomeUnknown`].
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for an out-of-bound header,
    /// [`ServiceError::Cancelled`] before any byte was written, and
    /// [`ServiceError::OutcomeUnknown`] once bytes may have reached the target.
    pub async fn write_open(&mut self, header: &[u8]) -> Result<(), ServiceError> {
        let wrote = async {
            self.frame(FrameKind::Open, header).await?;
            self.flush().await
        }
        .await;
        wrote.map_err(|error| {
            if error == ServiceError::Cancelled && self.wrote {
                ServiceError::OutcomeUnknown
            } else {
                error
            }
        })
    }

    /// Writes one frame of any type.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for an out-of-bound payload and
    /// [`ServiceError::Cancelled`] when the stream is gone.
    pub async fn frame(&mut self, kind: FrameKind, payload: &[u8]) -> Result<(), ServiceError> {
        let header = header(kind, payload.len())?;
        self.begin()?;
        self.write(&header).await?;
        self.write(payload).await?;
        self.torn = false;
        if matches!(kind, FrameKind::Outcome | FrameKind::Reject) {
            self.terminal_sent = true;
        }
        Ok(())
    }

    /// Writes one `data` frame; `bytes` must be 1..=64 KiB.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::frame`].
    pub async fn data(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), ServiceError> {
        if bytes.is_empty() || bytes.len() > MAX_DATA_CHUNK_LEN {
            return Err(ServiceError::InvalidInput);
        }
        let header = header(FrameKind::Data, bytes.len() + 1)?;
        self.begin()?;
        self.write(&header).await?;
        self.write(&[channel as u8]).await?;
        self.write(bytes).await?;
        self.torn = false;
        Ok(())
    }

    /// Writes one `end` frame for a channel.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::frame`].
    pub async fn end(&mut self, channel: Channel) -> Result<(), ServiceError> {
        self.frame(FrameKind::End, &[channel as u8]).await
    }

    /// Writes one `reject` frame.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::frame`].
    pub async fn reject(&mut self, error: ServiceError) -> Result<(), ServiceError> {
        self.frame(FrameKind::Reject, &[error.code()]).await
    }

    /// Flushes buffered bytes.
    ///
    /// # Errors
    /// Returns [`ServiceError::Cancelled`] when the stream is gone.
    pub async fn flush(&mut self) -> Result<(), ServiceError> {
        self.inner
            .flush()
            .await
            .map_err(|_| ServiceError::Cancelled)
    }

    /// Flushes and half-closes the stream.
    ///
    /// # Errors
    /// Returns [`ServiceError::Cancelled`] when the stream is gone.
    pub async fn shutdown(&mut self) -> Result<(), ServiceError> {
        self.inner
            .shutdown()
            .await
            .map_err(|_| ServiceError::Cancelled)
    }

    /// Returns the wrapped half.
    pub fn into_inner(self) -> W {
        self.inner
    }

    /// Lends the wrapped half, for example to reset a stream abandoned mid-frame.
    pub const fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    /// Starts a frame; the flag is cleared only once the whole frame was written.
    const fn begin(&mut self) -> Result<(), ServiceError> {
        if self.torn {
            return Err(ServiceError::Cancelled);
        }
        self.torn = true;
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), ServiceError> {
        let mut rest = bytes;
        while !rest.is_empty() {
            let written = self
                .inner
                .write(rest)
                .await
                .map_err(|_| ServiceError::Cancelled)?;
            if written == 0 {
                return Err(ServiceError::Cancelled);
            }
            self.wrote = true;
            rest = rest.get(written..).unwrap_or_default();
        }
        Ok(())
    }
}

/// Encodes a frame header, before any byte of the frame is written.
fn header(kind: FrameKind, length: usize) -> Result<[u8; FRAME_HEADER_LEN], ServiceError> {
    FrameHeader::new(kind, length)
        .and_then(FrameHeader::encode)
        .map_err(|_| ServiceError::InvalidInput)
}

#[cfg(test)]
#[path = "frame_io_tests.rs"]
mod tests;
