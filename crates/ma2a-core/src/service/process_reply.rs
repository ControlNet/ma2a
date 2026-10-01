//! Process v1 `accept`, `control`, and terminal `outcome` payloads.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    EndReason, LogWindow, MAX_HEADER_FRAME_LEN, MAX_PROCESS_PAGE, ProcessDetails, ProcessId,
    ProcessState, ProcessSummary, ServiceError,
    cbor::{self, Writer},
};

/// What a Process operation reports when it is accepted.
#[expect(
    clippy::exhaustive_enums,
    reason = "Process accept forms are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessAccept {
    /// `start` is bound to this Process; `true` when an earlier attempt already created it.
    Started(ProcessId, bool),
    /// `logs` or `attach` reads from `start`; `true` when older output was already dropped.
    Reading {
        /// The Process state when reading began.
        state: ProcessState,
        /// The cursor of the first byte that follows.
        start: u64,
        /// The retained window when reading began.
        window: LogWindow,
        /// Whether the requested position was older than the retained window.
        truncated: bool,
    },
    /// Any other operation: nothing beyond admission to report.
    Admitted,
}

impl ProcessAccept {
    /// Encodes one `accept` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode(self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        match self {
            Self::Started(id, replayed) => {
                writer
                    .array(3)?
                    .uint(0)
                    .bytes(id.as_bytes())?
                    .boolean(replayed);
            }
            Self::Reading {
                state,
                start,
                window,
                truncated,
            } => {
                writer
                    .array(6)?
                    .uint(1)
                    .text(state.name())?
                    .uint(start)
                    .uint(window.first_available)
                    .uint(window.end)
                    .boolean(truncated);
            }
            Self::Admitted => {
                writer.array(1)?.uint(2);
            }
        }
        writer.finish(MAX_HEADER_FRAME_LEN)
    }

    /// Decodes one `accept` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let accept = match (decoder.array(6)?, decoder.uint()?) {
            (3, 0) => Self::Started(ProcessId::try_from(decoder.bytes(16)?)?, decoder.boolean()?),
            (6, 1) => {
                let state = ProcessState::from_name(decoder.text(16)?)
                    .ok_or(ProtocolError::INVALID_INPUT)?;
                let start = decoder.uint()?;
                let (first_available, end) = (decoder.uint()?, decoder.uint()?);
                if first_available > end || start > end {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                Self::Reading {
                    state,
                    start,
                    window: LogWindow::new(first_available, end),
                    truncated: decoder.boolean()?,
                }
            }
            (1, 2) => Self::Admitted,
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        decoder.finish()?;
        Ok(accept)
    }
}

/// Encodes the attach `control` that reports output dropped before a slow reader got it.
///
/// # Errors
/// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
pub fn encode_skipped(cursor: u64) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = Writer::new()?;
    writer.array(2)?.uint(1).uint(cursor);
    writer.finish(MAX_HEADER_FRAME_LEN)
}

/// Decodes the attach `control` written by [`encode_skipped`].
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] for any other payload.
pub fn decode_skipped(bytes: &[u8]) -> Result<u64, ProtocolError> {
    let mut decoder = Decoder::new(bytes);
    decoder.array_exact(2)?;
    decoder.key(1)?;
    let cursor = decoder.uint()?;
    decoder.finish()?;
    Ok(cursor)
}

/// Successful terminal result of one Process operation.
#[expect(
    clippy::exhaustive_enums,
    reason = "Process results are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessReply {
    /// `start`, `signal`, or `terminate`: the Process as it now is.
    Summary(ProcessSummary),
    /// `list`: one page and the identifier to continue after, if more exist.
    Page(Vec<ProcessSummary>, Option<ProcessId>),
    /// `inspect`: the full view.
    Details(Box<ProcessDetails>),
    /// `logs`: the cursor to continue from; `true` once the Process ended and all was read.
    Logs(u64, bool),
    /// `attach` ended: why, the Process as it now is, and the cursor reached.
    Detached(EndReason, ProcessSummary, u64),
    /// `stdin`: bytes delivered, and whether the Process stdin is now closed.
    Written(u64, bool),
    /// `delete`: the Process is archived.
    Deleted,
}

/// Terminal Process outcome.
pub type ProcessReplyOutcome = Result<ProcessReply, ServiceError>;

impl ProcessReply {
    /// Encodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the outcome exceeds its bounds.
    pub fn encode_outcome(outcome: &ProcessReplyOutcome) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(outcome.as_ref().map_err(|error| *error), Self::encode)
    }

    /// Decodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(bytes: &[u8]) -> Result<ProcessReplyOutcome, ProtocolError> {
        cbor::decode_outcome(bytes, Self::decode)
    }

    fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Summary(summary) => {
                writer.array(2)?.uint(0);
                summary.encode(writer)?;
            }
            Self::Page(entries, next) => {
                writer.array(3)?.uint(1).array(entries.len())?;
                for entry in entries {
                    entry.encode(writer)?;
                }
                match next {
                    Some(id) => writer.bytes(id.as_bytes())?,
                    None => writer.null(),
                };
            }
            Self::Details(details) => {
                writer.array(2)?.uint(2);
                details.encode(writer)?;
            }
            Self::Logs(next, ended) => {
                writer.array(3)?.uint(3).uint(*next).boolean(*ended);
            }
            Self::Detached(reason, summary, cursor) => {
                writer.array(4)?.uint(4).text(reason.name())?;
                summary.encode(writer)?;
                writer.uint(*cursor);
            }
            Self::Written(bytes, closed) => {
                writer.array(3)?.uint(5).uint(*bytes).boolean(*closed);
            }
            Self::Deleted => {
                writer.array(1)?.uint(6);
            }
        }
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let reply = match (decoder.array(4)?, decoder.uint()?) {
            (2, 0) => Self::Summary(ProcessSummary::decode(decoder)?),
            (3, 1) => {
                let count = decoder.array(MAX_PROCESS_PAGE)?;
                let entries = (0..count)
                    .map(|_| ProcessSummary::decode(decoder))
                    .collect::<Result<Vec<_>, _>>()?;
                let next = if decoder.null() {
                    None
                } else {
                    Some(ProcessId::try_from(decoder.bytes(16)?)?)
                };
                Self::Page(entries, next)
            }
            (2, 2) => Self::Details(Box::new(ProcessDetails::decode(decoder)?)),
            (3, 3) => Self::Logs(decoder.uint()?, decoder.boolean()?),
            (4, 4) => {
                let reason = end_reason(decoder.text(16)?)?;
                let summary = ProcessSummary::decode(decoder)?;
                Self::Detached(reason, summary, decoder.uint()?)
            }
            (3, 5) => Self::Written(decoder.uint()?, decoder.boolean()?),
            (1, 6) => Self::Deleted,
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(reply)
    }
}

fn end_reason(name: &str) -> Result<EndReason, ProtocolError> {
    [
        EndReason::Completed,
        EndReason::TimedOut,
        EndReason::Cancelled,
        EndReason::Shutdown,
    ]
    .into_iter()
    .find(|reason| reason.name() == name)
    .ok_or(ProtocolError::INVALID_INPUT)
}
