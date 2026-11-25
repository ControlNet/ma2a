//! Caller input written to a program that may stop taking it.
//!
//! While one write waits for the program, the caller's next frame is still read, so a `cancel`,
//! a closed or lost stream right behind input the program does not take is seen at once: a
//! caller keeps its side open until the terminal, so closing it early cancels. The end of
//! input is kept and reading goes on past it, since a caller that ended its input has only a
//! `cancel` left to send. Any other frame is held until the write finishes, and the stream is
//! not read past it; a write still waiting [`INPUT_STALL`] after that ends the operation as
//! cancelled, so a `cancel` further back is never stuck behind it for longer.

use std::time::Duration;

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use tokio::{io::AsyncRead, sync::oneshot};

use crate::{Frame, FrameReader};

/// How long a write may still wait once a caller frame is held behind it.
pub(crate) const INPUT_STALL: Duration = Duration::from_secs(30);

/// The caller's frames, with at most the end of input and one more read ahead while a write
/// was waiting.
pub(crate) struct Input<'a, R> {
    reader: &'a mut FrameReader<R>,
    ended: Option<Frame>,
    ahead: Option<Frame>,
}

/// How one write of caller input ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Wrote<T> {
    /// The write finished with this result.
    Done(T),
    /// The caller cancelled, its stream failed, or the write stalled; the run must stop.
    Stopped(ServiceError),
    /// The run finished meanwhile.
    Finished,
}

impl<'a, R: AsyncRead + Unpin> Input<'a, R> {
    pub(crate) const fn new(reader: &'a mut FrameReader<R>) -> Self {
        Self {
            reader,
            ended: None,
            ahead: None,
        }
    }

    /// The next caller frame: the one held, if any, or the next one read. Cancel-safe.
    pub(crate) async fn next(&mut self) -> Result<Option<Frame>, ServiceError> {
        if let Some(frame) = self.ended.take().or_else(|| self.ahead.take()) {
            return Ok(Some(frame));
        }
        self.reader.next().await
    }

    /// Runs one write of caller input until it finishes, the caller stops the run, or the run
    /// finishes.
    pub(crate) async fn write<F: Future>(
        &mut self,
        write: F,
        finished: &mut oneshot::Receiver<()>,
    ) -> Wrote<F::Output> {
        tokio::pin!(write);
        let stall = tokio::time::sleep(Duration::MAX);
        tokio::pin!(stall);
        loop {
            tokio::select! {
                biased;
                _ = &mut *finished => return Wrote::Finished,
                written = &mut write => return Wrote::Done(written),
                frame = self.reader.next(), if self.ahead.is_none() => match frame {
                    Ok(Some(frame)) if frame.kind() == FrameKind::Cancel => {
                        return Wrote::Stopped(ServiceError::Cancelled);
                    }
                    Err(error) => return Wrote::Stopped(error),
                    // Only a `cancel` may follow the end of input, so it does not bound the write.
                    Ok(Some(frame))
                        if self.ended.is_none() && frame.end() == Ok(Channel::Input) =>
                    {
                        self.ended = Some(frame);
                    }
                    Ok(Some(frame)) => {
                        stall.as_mut().reset(tokio::time::Instant::now() + INPUT_STALL);
                        self.ahead = Some(frame);
                    }
                    Ok(None) => return Wrote::Stopped(ServiceError::Cancelled),
                },
                () = &mut stall => return Wrote::Stopped(ServiceError::Cancelled),
            }
        }
    }

    /// Runs one write within `bound` while still reading ahead for `cancel` or stream loss.
    ///
    /// Unlike [`Self::write`], a held frame does not shorten the wait: `bound` alone limits how
    /// long the write may stall, returning `None` when it expires.
    pub(crate) async fn write_within<F: Future>(
        &mut self,
        write: F,
        bound: Duration,
    ) -> Option<Result<F::Output, ServiceError>> {
        tokio::pin!(write);
        let deadline = tokio::time::sleep(bound);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                biased;
                written = &mut write => return Some(Ok(written)),
                () = &mut deadline => return None,
                frame = self.reader.next(), if self.ahead.is_none() => match frame {
                    Ok(Some(frame)) if frame.kind() == FrameKind::Cancel => {
                        return Some(Err(ServiceError::Cancelled));
                    }
                    Err(error) => return Some(Err(error)),
                    Ok(Some(frame))
                        if self.ended.is_none() && frame.end() == Ok(Channel::Input) =>
                    {
                        self.ended = Some(frame);
                    }
                    Ok(Some(_)) if self.ended.is_some() => {
                        return Some(Err(ServiceError::InvalidInput));
                    }
                    Ok(Some(frame)) => self.ahead = Some(frame),
                    Ok(None) => return Some(Err(ServiceError::Cancelled)),
                },
            }
        }
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
