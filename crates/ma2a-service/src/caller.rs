//! Discovering and calling services on another Endpoint through this user's Runtime.
//!
//! The caller never holds an Endpoint identity: every operation upgrades one local API
//! connection, and the Runtime dials the target as itself and relays the frames.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        Channel, DescribePage, FrameKind, HostedCall, OpenBody, OpenHeader, ServiceError,
        ServiceRequest, decode_call_accept, decode_call_outcome,
    },
};
use ma2a_runtime::ipc::{IpcPaths, LocalApiClient, LocalStream, StreamUpgrade};
use ma2a_services::{Frame, FrameReader, FrameWriter};
use serde_json::{Map, json};
use tokio::io::{AsyncWrite, ReadHalf, WriteHalf};

use crate::{
    Error,
    call::{Party, sends_on},
    wire,
};

/// What the service sent on one call.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Received {
    /// The service accepted the call with this header; it may now have effects.
    Accepted(Vec<u8>),
    /// Bytes on one channel; output is channel 1 and errors channel 2.
    Data(Channel, Vec<u8>),
    /// The service finished one channel.
    End(Channel),
    /// One out-of-band control message.
    Control(Vec<u8>),
    /// The call was refused and had no effect.
    Rejected(ServiceError),
    /// The call ended: its result, or its failure.
    Outcome(Result<Vec<u8>, ServiceError>),
}

/// The caller's side of one call.
#[derive(Debug)]
pub struct CallStream {
    reader: FrameReader<ReadHalf<LocalStream>>,
    writer: FrameWriter<WriteHalf<LocalStream>>,
    /// What the service sent so far.
    seen: Seen,
}

/// What the service sent so far on one call.
#[derive(Debug, Default)]
struct Seen {
    accepted: bool,
    /// Whether the service ended its output and its error channel.
    ended: (bool, bool),
    /// Whether the call ended with a terminal.
    finished: bool,
}

impl CallStream {
    /// Waits for the service's next frame; [`Received::Rejected`] and
    /// [`Received::Outcome`] are terminal, and nothing follows them.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] for a malformed frame or once the call ended, and
    /// [`Error::Service`] when the connection ends without a terminal.
    pub async fn next(&mut self) -> Result<Received, Error> {
        if self.seen.finished {
            return Err(Error::Protocol("the call already ended"));
        }
        let frame = self.reader.expect().await?;
        received(frame, &mut self.seen)
    }

    /// Sends 1–64 KiB on one channel; input is channel 0, the only one a caller sends on.
    ///
    /// # Errors
    /// Returns [`Error::Protocol`] for another channel, sending nothing, and
    /// [`Error::Service`] when the bytes are out of bounds or the connection is gone.
    pub async fn send(&mut self, channel: Channel, bytes: &[u8]) -> Result<(), Error> {
        sends_on(channel, Party::Caller)?;
        self.writer.data(channel, bytes).await?;
        Ok(self.writer.flush().await?)
    }

    /// Finishes one channel.
    ///
    /// # Errors
    /// The same as [`Self::send`].
    pub async fn end(&mut self, channel: Channel) -> Result<(), Error> {
        sends_on(channel, Party::Caller)?;
        self.writer.end(channel).await?;
        Ok(self.writer.flush().await?)
    }

    /// Sends one control message of 1 byte to 16 KiB; the service reads it as
    /// [`crate::Input::Control`].
    ///
    /// # Errors
    /// Returns [`Error::Service`] when the message is out of bounds or the connection is gone.
    pub async fn control(&mut self, message: &[u8]) -> Result<(), Error> {
        control(&mut self.writer, message).await
    }

    /// Asks the service to stop; keep reading for its terminal.
    ///
    /// # Errors
    /// Returns [`Error::Service`] when the connection is gone.
    pub async fn cancel(&mut self) -> Result<(), Error> {
        self.writer.frame(FrameKind::Cancel, &[]).await?;
        Ok(self.writer.flush().await?)
    }
}

/// What one service frame means to a call after what the service sent so far, which it
/// updates.
///
/// Services v1 allows `reject` only before `accept`, `data`, `end`, and `control` only after
/// it, and `data` and `end` from the service only on its output and error channels, each until
/// that channel's `end`; `accept` comes at most once, and nothing after a terminal. A frame out
/// of that order is malformed rather than taken: a `reject` after `accept` would say an accepted
/// call had no effect.
fn received(frame: Frame, seen: &mut Seen) -> Result<Received, Error> {
    let malformed = Error::Protocol("the service sent a malformed frame");
    let out_of_order = || Error::Protocol("the service sent a frame out of order");
    let in_order = match frame.kind() {
        _ if seen.finished => false,
        FrameKind::Accept | FrameKind::Reject => !seen.accepted,
        FrameKind::Data | FrameKind::End | FrameKind::Control => seen.accepted,
        _ => true,
    };
    if !in_order {
        return Err(out_of_order());
    }
    // The channel a `data` or `end` is on must still be open.
    let mut open = |channel, ends| {
        sends_on(channel, Party::Service).map_err(|_| out_of_order())?;
        let ended = if channel == Channel::Output {
            &mut seen.ended.0
        } else {
            &mut seen.ended.1
        };
        if *ended {
            return Err(out_of_order());
        }
        *ended = ends;
        Ok(channel)
    };
    Ok(match frame.kind() {
        FrameKind::Accept => {
            let header = decode_call_accept(frame.payload()).map_err(|_| malformed)?;
            seen.accepted = true;
            Received::Accepted(header)
        }
        FrameKind::Data => {
            let (channel, bytes) = frame.data()?;
            Received::Data(open(channel, false)?, bytes.to_vec())
        }
        FrameKind::End => Received::End(open(frame.end()?, true)?),
        FrameKind::Control => Received::Control(frame.into_payload()),
        FrameKind::Reject => {
            seen.finished = true;
            Received::Rejected(frame.rejection()?)
        }
        FrameKind::Outcome => {
            seen.finished = true;
            Received::Outcome(decode_call_outcome(frame.payload()).map_err(|_| malformed)?)
        }
        _ => return Err(malformed),
    })
}

/// Writes one `control` frame; its bounds are the frame header's.
async fn control<W: AsyncWrite + Unpin>(
    writer: &mut FrameWriter<W>,
    message: &[u8],
) -> Result<(), Error> {
    writer.frame(FrameKind::Control, message).await?;
    Ok(writer.flush().await?)
}

/// Longest wait for a target to answer one `describe`, dial included.
const DESCRIBE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// `page` when it continues a listing from `after`: a page repeating its cursor would keep a
/// caller paging forever, and one skipping past its last service would hide services.
fn continuing(page: DescribePage, after: Option<&str>) -> Result<DescribePage, Error> {
    if page.continues(after) {
        Ok(page)
    } else {
        Err(Error::Protocol("the target sent a page out of order"))
    }
}

/// Discovers and calls services on other Endpoints through this user's Runtime.
#[derive(Clone, Debug)]
pub struct Caller {
    client: LocalApiClient,
}

impl Caller {
    /// Creates a caller for the Runtime at `paths`.
    pub const fn new(paths: IpcPaths) -> Self {
        Self {
            client: LocalApiClient::new(paths),
        }
    }

    /// Returns one page of what `target` serves this Endpoint, after `after`.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] when the local Runtime refuses, [`Error::Service`] when
    /// the target refuses, such as `unauthorized` for an Endpoint sharing no Space, and
    /// [`Error::Protocol`] for a page that does not continue the listing from `after`.
    pub async fn describe(
        &self,
        target: EndpointId,
        after: Option<&str>,
    ) -> Result<DescribePage, Error> {
        let request = ServiceRequest::Describe(after.map(str::to_owned));
        let answered = async {
            let mut stream = self.open((target, wire::request_id()?), request).await?;
            let frame = stream.reader.expect().await?;
            match frame.kind() {
                FrameKind::Reject => Err(Error::Service(frame.rejection()?)),
                FrameKind::Outcome => DescribePage::decode_outcome(frame.payload())
                    .map_err(|_| Error::Protocol("the target sent a malformed page"))?
                    .map_err(Error::Service)
                    .and_then(|page| continuing(page, after)),
                _ => Err(Error::Protocol(
                    "describe is answered by one outcome or reject",
                )),
            }
        };
        tokio::time::timeout(DESCRIBE_DEADLINE, answered)
            .await
            .unwrap_or(Err(Error::Service(ServiceError::TimedOut)))
    }

    /// Opens one call to a service of `target` under `request_id`: retrying with the same
    /// ID never acts twice, and the same ID with a different call is a conflict.
    ///
    /// # Errors
    /// Returns [`Error::Refused`] when the local Runtime refuses the stream.
    pub async fn call(
        &self,
        (target, request_id): (EndpointId, RequestId),
        call: HostedCall,
    ) -> Result<CallStream, Error> {
        self.open((target, request_id), ServiceRequest::Call(call))
            .await
    }

    async fn open(
        &self,
        (target, request_id): (EndpointId, RequestId),
        request: ServiceRequest,
    ) -> Result<CallStream, Error> {
        let mut fields = Map::new();
        fields.insert("operation".to_owned(), json!("stream_open"));
        fields.insert(
            "request_id".to_owned(),
            json!(wire::hex(request_id.as_bytes())),
        );
        fields.insert(
            "target_endpoint_id".to_owned(),
            json!(wire::hex(target.as_bytes())),
        );
        fields.insert("service".to_owned(), json!("service"));
        let stream = match self.client.open_stream(&wire::command(fields)?).await? {
            StreamUpgrade::Opened(stream) => stream,
            StreamUpgrade::Refused(response) => {
                wire::payload(&response, "stream_opened")?;
                return Err(Error::Protocol("the Runtime refused without an error"));
            }
            _ => return Err(Error::Protocol("the Runtime answered an unknown upgrade")),
        };
        let (read, write) = tokio::io::split(stream);
        let mut opened = CallStream {
            reader: FrameReader::new(read),
            writer: FrameWriter::new(write),
            seen: Seen::default(),
        };
        let header = OpenHeader::new(request_id, target, OpenBody::Service(request))
            .encode()
            .map_err(|_| Error::Protocol("the request exceeds a protocol bound"))?;
        opened.writer.frame(FrameKind::Open, &header).await?;
        opened.writer.flush().await?;
        Ok(opened)
    }
}

#[cfg(test)]
#[path = "caller_tests.rs"]
mod tests;
