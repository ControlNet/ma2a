//! One admitted call, as its service sees it.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        Channel, FrameKind, OperationName, ServiceError, encode_call_accept, encode_call_outcome,
    },
};
use ma2a_runtime::ipc::LocalStream;
use ma2a_services::{FrameReader, FrameWriter};
use tokio::io::{AsyncRead, ReadHalf, WriteHalf};

use crate::Error;

/// Proof that a call was answered with its one terminal frame.
#[derive(Debug)]
#[must_use]
pub struct Ended(());

/// What the caller sent.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Input {
    /// Bytes on one channel; input is channel 0.
    Data(Channel, Vec<u8>),
    /// The caller finished one channel.
    End(Channel),
    /// One out-of-band control message.
    Control(Vec<u8>),
    /// The caller cancelled, left, or the Runtime withdrew the call. Answer it with
    /// [`CallOutput::fail`] and [`ServiceError::Cancelled`] after undoing what can be undone.
    Cancelled,
}

/// The caller-to-service frames of one call.
#[derive(Debug)]
pub struct CallInput(FrameReader<ReadHalf<LocalStream>>, Channels);

impl CallInput {
    /// Waits for the caller's next frame. Safe to use in `tokio::select!`.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] for a frame the caller may not send, such as data on
    /// another channel than input, or after its `end` of input.
    pub async fn recv(&mut self) -> Result<Input, Error> {
        caller_input(&mut self.0, &mut self.1).await
    }
}

/// Reads the caller's next frame, given the channels it `ended`; see [`CallInput::recv`].
pub(crate) async fn caller_input<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    ended: &mut Channels,
) -> Result<Input, Error> {
    let Ok(Some(frame)) = reader.next().await else {
        return Ok(Input::Cancelled);
    };
    let malformed = |_| Error::Protocol("the caller sent a malformed frame");
    match frame.kind() {
        FrameKind::Data => {
            let (channel, bytes) = frame.data().map_err(malformed)?;
            ended.take((channel, Party::Caller), false)?;
            Ok(Input::Data(channel, bytes.to_vec()))
        }
        FrameKind::End => {
            let channel = frame.end().map_err(malformed)?;
            ended.take((channel, Party::Caller), true)?;
            Ok(Input::End(channel))
        }
        FrameKind::Control => Ok(Input::Control(frame.into_payload())),
        FrameKind::Cancel => Ok(Input::Cancelled),
        _ => Err(Error::Protocol("the caller sent a frame it may not send")),
    }
}

/// Who sends one frame of a call.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Party {
    Caller,
    Service,
}

/// Checks that `party` may send `data` or `end` on `channel`: the caller sends only input, on
/// channel 0, and the service only output and errors, on channels 1 and 2.
pub(crate) const fn sends_on(channel: Channel, party: Party) -> Result<(), Error> {
    match (party, channel) {
        (Party::Caller, Channel::Input) | (Party::Service, Channel::Output | Channel::Error) => {
            Ok(())
        }
        (Party::Caller, _) => Err(Error::Protocol(
            "the caller sends only on the input channel",
        )),
        (Party::Service, _) => Err(Error::Protocol(
            "the service sends only on the output and error channels",
        )),
    }
}

/// The channels one party ended: it sends nothing more on them.
#[derive(Debug, Default)]
pub(crate) struct Channels {
    input: bool,
    output: bool,
    error: bool,
}

impl Channels {
    /// Checks that `party` may send `data` on `channel`, or its `end` when `ends`, which it
    /// records.
    pub(crate) fn take(
        &mut self,
        (channel, party): (Channel, Party),
        ends: bool,
    ) -> Result<(), Error> {
        sends_on(channel, party)?;
        let ended = match channel {
            Channel::Input => &mut self.input,
            Channel::Output => &mut self.output,
            Channel::Error => &mut self.error,
        };
        if *ended {
            return Err(Error::Protocol("nothing follows the end of a channel"));
        }
        *ended = ends;
        Ok(())
    }
}

/// The service-to-caller frames of one call.
#[derive(Debug)]
pub struct CallOutput {
    writer: FrameWriter<WriteHalf<LocalStream>>,
    accepted: bool,
    ended: Channels,
}

impl CallOutput {
    /// Accepts the call with an opaque header of at most 8 KiB: from now on the call may have
    /// effects, and it can only end with an outcome.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] when already accepted or the header is too long, and
    /// [`Error::Service`] when the connection is gone.
    pub async fn accept(&mut self, header: &[u8]) -> Result<(), Error> {
        if self.accepted {
            return Err(Error::Protocol("the call is already accepted"));
        }
        let payload =
            encode_call_accept(header).map_err(|_| Error::Protocol("the header is too long"))?;
        self.writer.frame(FrameKind::Accept, &payload).await?;
        self.writer.flush().await?;
        self.accepted = true;
        Ok(())
    }

    /// Sends 1–64 KiB on one channel; output is channel 1 and errors channel 2.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] before `accept`, on the input channel, or after that
    /// channel's `end`, sending nothing, and [`Error::Service`] when the bytes are out of
    /// bounds or the connection is gone.
    pub async fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), Error> {
        self.require_accepted()?;
        self.ended.take((channel, Party::Service), false)?;
        self.writer.data(channel, bytes).await?;
        Ok(self.writer.flush().await?)
    }

    /// Finishes one channel.
    ///
    /// # Errors
    /// The same as [`Self::send`].
    pub async fn end(&mut self, channel: Channel) -> Result<(), Error> {
        self.require_accepted()?;
        self.ended.take((channel, Party::Service), true)?;
        self.writer.end(channel).await?;
        Ok(self.writer.flush().await?)
    }

    /// Sends one control message of at most 16 KiB.
    ///
    /// # Errors
    /// The same as [`Self::send`].
    pub async fn control(&mut self, message: &[u8]) -> Result<(), Error> {
        self.require_accepted()?;
        self.writer.frame(FrameKind::Control, message).await?;
        Ok(self.writer.flush().await?)
    }

    /// Ends the call with its result, at most 60,000 bytes.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] for a longer result, and [`Error::Service`] when the
    /// connection is gone, so the caller learns `outcome_unknown`.
    pub async fn reply(self, result: &[u8]) -> Result<Ended, Error> {
        let payload = encode_call_outcome(Ok(result))
            .map_err(|_| Error::Protocol("the result is too long"))?;
        self.terminal(FrameKind::Outcome, &payload).await
    }

    /// Ends the call with a failure after it may have had effects.
    ///
    /// # Errors
    /// Returns [`Error::Service`] when the connection is gone.
    pub async fn fail(self, error: ServiceError) -> Result<Ended, Error> {
        let payload =
            encode_call_outcome(Err(error)).map_err(|_| Error::Protocol("unencodable failure"))?;
        self.terminal(FrameKind::Outcome, &payload).await
    }

    /// Refuses the call before `accept`, promising it had no effect.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] after `accept`, and [`Error::Service`] when the connection
    /// is gone.
    pub async fn reject(self, error: ServiceError) -> Result<Ended, Error> {
        if self.accepted {
            return Err(Error::Protocol("an accepted call ends with an outcome"));
        }
        self.terminal(FrameKind::Reject, &[error.code()]).await
    }

    async fn terminal(mut self, kind: FrameKind, payload: &[u8]) -> Result<Ended, Error> {
        self.writer.frame(kind, payload).await?;
        self.writer.shutdown().await?;
        Ok(Ended(()))
    }

    const fn require_accepted(&self) -> Result<(), Error> {
        if self.accepted {
            Ok(())
        } else {
            Err(Error::Protocol("accept the call first"))
        }
    }
}

/// One call the Runtime admitted and handed to this service.
///
/// Dropping a call without a terminal frame leaves the caller with `outcome_unknown`.
#[derive(Debug)]
pub struct Call {
    caller: EndpointId,
    request_id: RequestId,
    version: u16,
    operation: OperationName,
    body: Vec<u8>,
    input: CallInput,
    output: CallOutput,
}

impl Call {
    pub(crate) fn new(
        (caller, request_id): (EndpointId, RequestId),
        (version, operation, body): (u16, OperationName, Vec<u8>),
        stream: LocalStream,
    ) -> Self {
        let (read, write) = tokio::io::split(stream);
        Self {
            caller,
            request_id,
            version,
            operation,
            body,
            input: CallInput(FrameReader::new(read), Channels::default()),
            output: CallOutput {
                writer: FrameWriter::new(write),
                accepted: false,
                ended: Channels::default(),
            },
        }
    }

    /// The authenticated Endpoint that called.
    pub const fn caller(&self) -> EndpointId {
        self.caller
    }

    /// The caller's request ID: the same ID means the same request, so effects keyed by
    /// `(caller, request_id)` happen at most once.
    pub const fn request_id(&self) -> RequestId {
        self.request_id
    }

    /// The exact version the caller chose.
    pub const fn version(&self) -> u16 {
        self.version
    }

    /// The operation within that version.
    pub const fn operation(&self) -> &OperationName {
        &self.operation
    }

    /// The opaque request body, at most 8 KiB.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Splits the call so input and output can be used at once, for example to stop
    /// streaming as soon as the caller cancels.
    pub fn split(self) -> (CallInput, CallOutput) {
        (self.input, self.output)
    }

    /// Ends an unary call with its result without accepting it first.
    ///
    /// # Errors
    /// The same as [`CallOutput::reply`].
    pub async fn reply(self, result: &[u8]) -> Result<Ended, Error> {
        self.output.reply(result).await
    }

    /// Ends the call with a failure.
    ///
    /// # Errors
    /// The same as [`CallOutput::fail`].
    pub async fn fail(self, error: ServiceError) -> Result<Ended, Error> {
        self.output.fail(error).await
    }

    /// Refuses the call with no effect.
    ///
    /// # Errors
    /// The same as [`CallOutput::reject`].
    pub async fn reject(self, error: ServiceError) -> Result<Ended, Error> {
        self.output.reject(error).await
    }
}

#[cfg(test)]
#[path = "call_tests.rs"]
mod tests;
