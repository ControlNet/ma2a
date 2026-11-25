//! The durable state of one Process as its creator sees it.

use crate::{ProtocolError, RequestId, space_codec::Decoder};

use super::{
    LogFrom, ProcessId, ProcessReason, ProcessStart, ProcessState, Termination,
    cbor::{self, Writer},
    process_start::MAX_PROCESS_NAME_LEN,
};

/// The content-free view of one Process.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSummary {
    /// Process identifier.
    pub id: ProcessId,
    /// `RequestId` of the `start` that created it.
    pub request_id: RequestId,
    /// Lifecycle state.
    pub state: ProcessState,
    /// Why it ended, for `terminated`, `failed`, and `lost`.
    pub reason: Option<ProcessReason>,
    /// How the child ended, when observed.
    pub termination: Option<Termination>,
    /// Display name, if any.
    pub name: Option<String>,
    /// Creation, start, and end times in Unix milliseconds.
    pub times: (u64, Option<u64>, Option<u64>),
}

impl ProcessSummary {
    /// Creates a summary of a just-created Process.
    pub const fn new(id: ProcessId, request_id: RequestId, created_ms: u64) -> Self {
        Self {
            id,
            request_id,
            state: ProcessState::Created,
            reason: None,
            termination: None,
            name: None,
            times: (created_ms, None, None),
        }
    }

    /// Returns this summary in `state` with its end detail.
    #[must_use]
    pub const fn with_state(
        mut self,
        state: ProcessState,
        (reason, termination): (Option<ProcessReason>, Option<Termination>),
    ) -> Self {
        self.state = state;
        self.reason = reason;
        self.termination = termination;
        self
    }

    /// Returns this summary with a display name and start and end times.
    #[must_use]
    pub fn with_details(
        mut self,
        name: Option<String>,
        (started_ms, ended_ms): (Option<u64>, Option<u64>),
    ) -> Self {
        self.name = name;
        self.times = (self.times.0, started_ms, ended_ms);
        self
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(9)?
            .bytes(self.id.as_bytes())?
            .bytes(self.request_id.as_bytes())?
            .uint(self.state.code())
            .optional_uint(self.reason.map(ProcessReason::code));
        match self.termination {
            Some(termination) => termination.encode(writer)?,
            None => {
                writer.null();
            }
        }
        writer
            .optional_text(self.name.as_deref())?
            .uint(self.times.0)
            .optional_uint(self.times.1)
            .optional_uint(self.times.2);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(9)?;
        let id = ProcessId::try_from(decoder.bytes(16)?)?;
        let request_id = RequestId::try_from(decoder.bytes(16)?)?;
        let state = ProcessState::from_code(decoder.uint()?)?;
        let reason = cbor::optional_uint(decoder)?
            .map(ProcessReason::from_code)
            .transpose()?;
        let termination = if decoder.null() {
            None
        } else {
            Some(Termination::decode(decoder)?)
        };
        let name = cbor::optional_text(decoder, MAX_PROCESS_NAME_LEN)?;
        let created_ms = decoder.uint()?;
        let times = (cbor::optional_uint(decoder)?, cbor::optional_uint(decoder)?);
        Ok(Self::new(id, request_id, created_ms)
            .with_state(state, (reason, termination))
            .with_details(name, times))
    }
}

/// The retained output window of one Process, as content-byte cursors.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogWindow {
    /// The first cursor still retained.
    pub first_available: u64,
    /// The cursor after the last byte written.
    pub end: u64,
}

impl LogWindow {
    /// Creates a window; `first_available` never exceeds `end`.
    pub const fn new(first_available: u64, end: u64) -> Self {
        let first_available = if first_available > end {
            end
        } else {
            first_available
        };
        Self {
            first_available,
            end,
        }
    }

    /// Where a read from `from` starts in this window, and whether output before that was
    /// already dropped; `None` for a cursor past `end`, which no read may name.
    pub const fn resolve(self, from: LogFrom) -> Option<(u64, bool)> {
        match from {
            LogFrom::Start => Some((self.first_available, false)),
            LogFrom::Now => Some((self.end, false)),
            LogFrom::Cursor(cursor) if cursor > self.end => None,
            LogFrom::Cursor(cursor) if cursor < self.first_available => {
                Some((self.first_available, true))
            }
            LogFrom::Cursor(cursor) => Some((cursor, false)),
        }
    }
}

/// The full view of one Process: its summary, its start, and its output window.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessDetails {
    /// Content-free summary.
    pub summary: ProcessSummary,
    /// The start it was created with, or `None` once archived.
    pub start: Option<ProcessStart>,
    /// Retained output window.
    pub log: LogWindow,
}

impl ProcessDetails {
    /// Creates a full view.
    pub const fn new(summary: ProcessSummary, start: Option<ProcessStart>, log: LogWindow) -> Self {
        Self {
            summary,
            start,
            log,
        }
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(4)?;
        self.summary.encode(writer)?;
        match &self.start {
            Some(start) => start.encode(writer)?,
            None => {
                writer.null();
            }
        }
        writer.uint(self.log.first_available).uint(self.log.end);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        let summary = ProcessSummary::decode(decoder)?;
        let start = if decoder.null() {
            None
        } else {
            Some(ProcessStart::decode(decoder)?)
        };
        let (first_available, end) = (decoder.uint()?, decoder.uint()?);
        if first_available > end {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self::new(
            summary,
            start,
            LogWindow::new(first_available, end),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{LogFrom, LogWindow};

    #[test]
    fn positions_resolve_inside_the_retained_window() {
        let window = LogWindow::new(100, 250);
        assert_eq!(window.resolve(LogFrom::Start), Some((100, false)));
        assert_eq!(window.resolve(LogFrom::Now), Some((250, false)));
        assert_eq!(window.resolve(LogFrom::Cursor(120)), Some((120, false)));
        assert_eq!(window.resolve(LogFrom::Cursor(250)), Some((250, false)));
        assert_eq!(window.resolve(LogFrom::Cursor(3)), Some((100, true)));
        assert_eq!(window.resolve(LogFrom::Cursor(251)), None);
    }
}
