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
#[derive(Debug)]
pub struct FrameReader<R> {
    inner: R,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Wraps one readable half.
    pub const fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Reads the next frame, or `None` at a clean end of stream between frames.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for an unknown type or out-of-bound length, and
    /// [`ServiceError::Cancelled`] when the stream ends or resets inside a frame.
    pub async fn next(&mut self) -> Result<Option<Frame>, ServiceError> {
        let mut header = [0_u8; FRAME_HEADER_LEN];
        let first = self
            .inner
            .read(&mut header)
            .await
            .map_err(|_| ServiceError::Cancelled)?;
        if first == 0 {
            return Ok(None);
        }
        let rest = header.get_mut(first..).ok_or(ServiceError::InvalidInput)?;
        self.inner
            .read_exact(rest)
            .await
            .map_err(|_| ServiceError::Cancelled)?;
        let header = FrameHeader::parse(header).map_err(|_| ServiceError::InvalidInput)?;
        let mut payload = vec![0_u8; header.length()];
        self.inner
            .read_exact(&mut payload)
            .await
            .map_err(|_| ServiceError::Cancelled)?;
        Ok(Some(Frame {
            kind: header.kind(),
            payload,
        }))
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
#[derive(Debug)]
pub struct FrameWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    /// Wraps one writable half.
    pub const fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Writes one frame of any type.
    ///
    /// # Errors
    /// Returns [`ServiceError::InvalidInput`] for an out-of-bound payload and
    /// [`ServiceError::Cancelled`] when the stream is gone.
    pub async fn frame(&mut self, kind: FrameKind, payload: &[u8]) -> Result<(), ServiceError> {
        self.header(kind, payload.len()).await?;
        self.write(payload).await
    }

    /// Writes one `data` frame; `bytes` must be 1..=64 KiB.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::frame`].
    pub async fn data(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), ServiceError> {
        if bytes.is_empty() || bytes.len() > MAX_DATA_CHUNK_LEN {
            return Err(ServiceError::InvalidInput);
        }
        self.header(FrameKind::Data, bytes.len() + 1).await?;
        self.write(&[channel as u8]).await?;
        self.write(bytes).await
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

    async fn header(&mut self, kind: FrameKind, length: usize) -> Result<(), ServiceError> {
        let header = FrameHeader::new(kind, length)
            .and_then(FrameHeader::encode)
            .map_err(|_| ServiceError::InvalidInput)?;
        self.write(&header).await
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), ServiceError> {
        self.inner
            .write_all(bytes)
            .await
            .map_err(|_| ServiceError::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{Channel, FrameKind, MAX_DATA_CHUNK_LEN, ServiceError};

    use super::{FrameReader, FrameWriter};

    #[tokio::test]
    async fn frames_roundtrip_and_a_clean_end_is_distinguished_from_truncation() {
        let (client, server) = tokio::io::duplex(1 << 20);
        let mut writer = FrameWriter::new(client);
        writer.data(Channel::Output, b"hello").await.expect("data");
        writer.end(Channel::Input).await.expect("end");
        writer.reject(ServiceError::NotFound).await.expect("reject");
        writer.frame(FrameKind::Cancel, &[]).await.expect("cancel");
        writer.shutdown().await.expect("shutdown");
        let mut reader = FrameReader::new(server);
        let data = reader.expect().await.expect("data frame");
        assert_eq!(
            data.data().expect("payload"),
            (Channel::Output, &b"hello"[..])
        );
        assert_eq!(
            reader.expect().await.expect("end").end(),
            Ok(Channel::Input)
        );
        assert_eq!(
            reader.expect().await.expect("reject").rejection(),
            Ok(ServiceError::NotFound)
        );
        assert_eq!(
            reader.expect().await.expect("cancel").kind(),
            FrameKind::Cancel
        );
        assert_eq!(reader.next().await, Ok(None));
    }

    #[tokio::test]
    async fn oversized_and_truncated_frames_are_refused_before_buffering() {
        let (mut client, server) = tokio::io::duplex(64);
        tokio::io::AsyncWriteExt::write_all(&mut client, &[4, 0, 0x10, 0, 2])
            .await
            .expect("header");
        drop(client);
        assert_eq!(
            FrameReader::new(server).next().await,
            Err(ServiceError::InvalidInput)
        );
        let (mut client, server) = tokio::io::duplex(64);
        tokio::io::AsyncWriteExt::write_all(&mut client, &[5, 0, 0, 0, 1])
            .await
            .expect("header");
        drop(client);
        assert_eq!(
            FrameReader::new(server).next().await,
            Err(ServiceError::Cancelled)
        );
        let (client, _server) = tokio::io::duplex(64);
        let oversized = vec![1_u8; MAX_DATA_CHUNK_LEN + 1];
        assert_eq!(
            FrameWriter::new(client)
                .data(Channel::Input, &oversized)
                .await,
            Err(ServiceError::InvalidInput)
        );
    }
}
