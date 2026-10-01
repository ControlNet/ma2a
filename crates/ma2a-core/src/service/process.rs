//! Process v1 requests: durable workloads hosted by the target Endpoint.
//!
//! A Process is a resource, never an identity. Its [`ProcessId`] names it within the Runtime
//! that hosts it, and only the Endpoint that created it may manage it remotely.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    ProcessId, ServiceAccess,
    cbor::{self, Writer},
    process_start::ProcessStart,
};

/// Most bytes one `logs` request returns.
pub const MAX_LOGS_READ: u32 = 1_048_576;
/// Longest grace period between `terminate` and `kill`.
pub const MAX_TERMINATE_GRACE_MS: u32 = 60_000;
/// Most Processes one `list` page returns.
pub const MAX_PROCESS_PAGE: usize = 32;

/// Where a `logs` or `attach` request starts reading retained output.
#[expect(
    clippy::exhaustive_enums,
    reason = "log positions are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFrom {
    /// The oldest retained byte.
    Start,
    /// The current end: only output produced from now on.
    Now,
    /// This exact content-byte cursor.
    Cursor(u64),
}

impl LogFrom {
    fn encode(self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Start => writer.array(1)?.uint(0),
            Self::Now => writer.array(1)?.uint(1),
            Self::Cursor(cursor) => writer.array(2)?.uint(2).uint(cursor),
        };
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        match (decoder.array(2)?, decoder.uint()?) {
            (1, 0) => Ok(Self::Start),
            (1, 1) => Ok(Self::Now),
            (2, 2) => Ok(Self::Cursor(decoder.uint()?)),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}

/// A portable signal a caller may send to a running Process.
#[expect(
    clippy::exhaustive_enums,
    reason = "signals are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessSignal {
    /// `SIGINT`.
    Interrupt,
    /// `SIGTERM`; on Windows the Process is ended.
    Terminate,
    /// `SIGKILL`; on Windows the Process is ended.
    Kill,
    /// `SIGHUP`.
    Hangup,
    /// `SIGQUIT`.
    Quit,
    /// `SIGUSR1`.
    User1,
    /// `SIGUSR2`.
    User2,
}

impl ProcessSignal {
    /// Every signal in wire-code order.
    pub const ALL: [Self; 7] = [
        Self::Interrupt,
        Self::Terminate,
        Self::Kill,
        Self::Hangup,
        Self::Quit,
        Self::User1,
        Self::User2,
    ];

    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Interrupt => "interrupt",
            Self::Terminate => "terminate",
            Self::Kill => "kill",
            Self::Hangup => "hangup",
            Self::Quit => "quit",
            Self::User1 => "user1",
            Self::User2 => "user2",
        }
    }

    /// Returns the signal with this name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|signal| signal.name() == name)
    }

    const fn code(self) -> u64 {
        match self {
            Self::Interrupt => 0,
            Self::Terminate => 1,
            Self::Kill => 2,
            Self::Hangup => 3,
            Self::Quit => 4,
            Self::User1 => 5,
            Self::User2 => 6,
        }
    }

    fn from_code(code: u64) -> Result<Self, ProtocolError> {
        Self::ALL
            .into_iter()
            .find(|signal| signal.code() == code)
            .ok_or(ProtocolError::INVALID_INPUT)
    }
}

/// One Process operation.
#[expect(
    clippy::exhaustive_enums,
    reason = "Process operations are a closed wire vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessRequest {
    /// Create and start one Process; the `RequestId` binds exactly one Process.
    Start(ProcessStart),
    /// Return one page of the caller's Processes after this identifier.
    List(Option<ProcessId>),
    /// Return one Process in full.
    Inspect(ProcessId),
    /// Return at most this many retained output bytes from a position.
    Logs(ProcessId, LogFrom, u32),
    /// Follow output from a position; `true` also takes the single stdin writer lease.
    Attach(ProcessId, LogFrom, bool),
    /// Write stdin until `end`; `true` then closes the Process stdin for good.
    Stdin(ProcessId, bool),
    /// Send one signal.
    Signal(ProcessId, ProcessSignal),
    /// Terminate, then kill after this grace period in milliseconds.
    Terminate(ProcessId, u32),
    /// Archive a terminal Process: its output and specification are removed.
    Delete(ProcessId),
}

impl ProcessRequest {
    /// Returns the complete access every Process operation needs.
    pub const fn access(&self) -> ServiceAccess {
        ServiceAccess::Process
    }

    /// Returns whether this operation can change target state.
    pub const fn is_mutation(&self) -> bool {
        match self {
            Self::List(_) | Self::Inspect(_) | Self::Logs(..) => false,
            Self::Attach(_, _, stdin) => *stdin,
            Self::Start(_)
            | Self::Stdin(..)
            | Self::Signal(..)
            | Self::Terminate(..)
            | Self::Delete(_) => true,
        }
    }

    /// Returns the Process an operation addresses, if it addresses one.
    pub const fn process_id(&self) -> Option<ProcessId> {
        match self {
            Self::Start(_) | Self::List(_) => None,
            Self::Inspect(id)
            | Self::Logs(id, ..)
            | Self::Attach(id, ..)
            | Self::Stdin(id, _)
            | Self::Signal(id, _)
            | Self::Terminate(id, _)
            | Self::Delete(id) => Some(*id),
        }
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Start(start) => {
                writer.array(2)?.uint(0);
                start.encode(writer)?;
            }
            Self::List(after) => {
                writer.array(2)?.uint(1);
                match after {
                    Some(id) => writer.bytes(id.as_bytes())?,
                    None => writer.null(),
                };
            }
            Self::Inspect(id) => id_only(writer, 2, *id)?,
            Self::Logs(id, from, max) => {
                id_then(writer, (4, 3), *id)?;
                from.encode(writer)?;
                writer.uint(u64::from(*max));
            }
            Self::Attach(id, from, stdin) => {
                id_then(writer, (4, 4), *id)?;
                from.encode(writer)?;
                writer.boolean(*stdin);
            }
            Self::Stdin(id, close) => {
                id_then(writer, (3, 5), *id)?.boolean(*close);
            }
            Self::Signal(id, signal) => {
                id_then(writer, (3, 6), *id)?.uint(signal.code());
            }
            Self::Terminate(id, grace) => {
                id_then(writer, (3, 7), *id)?.uint(u64::from(*grace));
            }
            Self::Delete(id) => id_only(writer, 8, *id)?,
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let request = match (decoder.array(4)?, decoder.uint()?) {
            (2, 0) => Self::Start(ProcessStart::decode(decoder)?),
            (2, 1) => Self::List(if decoder.null() {
                None
            } else {
                Some(ProcessId::decode_from(decoder)?)
            }),
            (2, 2) => Self::Inspect(ProcessId::decode_from(decoder)?),
            (4, 3) => {
                let (id, from) = (ProcessId::decode_from(decoder)?, LogFrom::decode(decoder)?);
                let max = cbor::u32_value(decoder)?;
                if max == 0 || max > MAX_LOGS_READ {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                Self::Logs(id, from, max)
            }
            (4, 4) => Self::Attach(
                ProcessId::decode_from(decoder)?,
                LogFrom::decode(decoder)?,
                decoder.boolean()?,
            ),
            (3, 5) => Self::Stdin(ProcessId::decode_from(decoder)?, decoder.boolean()?),
            (3, 6) => Self::Signal(
                ProcessId::decode_from(decoder)?,
                ProcessSignal::from_code(decoder.uint()?)?,
            ),
            (3, 7) => {
                let id = ProcessId::decode_from(decoder)?;
                let grace = cbor::u32_value(decoder)?;
                if grace > MAX_TERMINATE_GRACE_MS {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                Self::Terminate(id, grace)
            }
            (2, 8) => Self::Delete(ProcessId::decode_from(decoder)?),
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        Ok(request)
    }
}

fn id_only(writer: &mut Writer, tag: u64, id: ProcessId) -> Result<(), ProtocolError> {
    writer.array(2)?.uint(tag).bytes(id.as_bytes())?;
    Ok(())
}

fn id_then(
    writer: &mut Writer,
    (length, tag): (usize, u64),
    id: ProcessId,
) -> Result<&mut Writer, ProtocolError> {
    writer.array(length)?.uint(tag).bytes(id.as_bytes())
}
