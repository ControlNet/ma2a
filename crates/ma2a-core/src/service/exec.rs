//! Exec v1 and Shell v1 request, control, and result contracts.

use crate::{ProtocolError, space_codec::Decoder};

use super::{
    EndReason, ProcessSpec, ServiceError, Termination,
    cbor::{self, Writer},
};

/// Maximum Exec timeout and Shell lifetime: 24 hours.
pub const MAX_OPERATION_LIFETIME_MS: u64 = 86_400_000;
/// Maximum byte length of a Shell `TERM` value.
pub const MAX_TERM_LEN: usize = 64;

/// One non-interactive program execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecRequest {
    spec: ProcessSpec,
    stdin: bool,
    timeout_ms: u64,
}

impl ExecRequest {
    /// Validates an Exec request; the program is required.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] without a program or with an invalid timeout.
    pub fn new(spec: ProcessSpec, stdin: bool, timeout_ms: u64) -> Result<Self, ProtocolError> {
        if spec.program().is_none() || timeout_ms == 0 || timeout_ms > MAX_OPERATION_LIFETIME_MS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            spec,
            stdin,
            timeout_ms,
        })
    }

    /// Returns the program specification.
    pub const fn spec(&self) -> &ProcessSpec {
        &self.spec
    }

    /// Returns whether the client streams stdin; otherwise stdin is empty.
    pub const fn stdin(&self) -> bool {
        self.stdin
    }

    /// Returns the timeout after which the child is killed.
    pub const fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(3)?;
        self.spec.encode(writer)?;
        writer.boolean(self.stdin).uint(self.timeout_ms);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(3)?;
        let spec = ProcessSpec::decode(decoder)?;
        Self::new(spec, decoder.boolean()?, decoder.uint()?)
    }
}

/// Terminal dimensions in character cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    cols: u16,
    rows: u16,
}

impl TerminalSize {
    /// Validates non-zero dimensions.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when either dimension is zero.
    pub const fn new(cols: u16, rows: u16) -> Result<Self, ProtocolError> {
        if cols == 0 || rows == 0 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { cols, rows })
    }

    /// Returns the column count.
    pub const fn cols(self) -> u16 {
        self.cols
    }

    /// Returns the row count.
    pub const fn rows(self) -> u16 {
        self.rows
    }

    fn encode(self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(2)?
            .uint(u64::from(self.cols))
            .uint(u64::from(self.rows));
        Ok(())
    }

    fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(2)?;
        Self::new(cbor::u16_value(decoder)?, cbor::u16_value(decoder)?)
    }

    /// Encodes a Shell resize `control` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode_resize(self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        writer.array(2)?.uint(0);
        self.encode(&mut writer)?;
        writer.finish(super::MAX_HEADER_FRAME_LEN)
    }

    /// Decodes a Shell resize `control` payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_resize(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        decoder.array_exact(2)?;
        decoder.key(0)?;
        let size = Self::decode(&mut decoder)?;
        decoder.finish()?;
        Ok(size)
    }
}

/// One interactive terminal session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellRequest {
    spec: ProcessSpec,
    term: String,
    size: TerminalSize,
    lifetime_ms: u64,
}

impl ShellRequest {
    /// Validates a Shell request; a missing program selects the target's default shell.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an invalid `TERM` or lifetime.
    pub fn new(
        spec: ProcessSpec,
        terminal: (String, TerminalSize),
        lifetime_ms: u64,
    ) -> Result<Self, ProtocolError> {
        let (term, size) = terminal;
        let valid_term = !term.is_empty()
            && term.len() <= MAX_TERM_LEN
            && term.bytes().all(|byte| byte.is_ascii_graphic());
        if !valid_term || lifetime_ms == 0 || lifetime_ms > MAX_OPERATION_LIFETIME_MS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            spec,
            term,
            size,
            lifetime_ms,
        })
    }

    /// Returns the program specification.
    pub const fn spec(&self) -> &ProcessSpec {
        &self.spec
    }

    /// Returns the requested `TERM` value.
    pub fn term(&self) -> &str {
        &self.term
    }

    /// Returns the initial terminal size.
    pub const fn size(&self) -> TerminalSize {
        self.size
    }

    /// Returns the maximum session lifetime.
    pub const fn lifetime_ms(&self) -> u64 {
        self.lifetime_ms
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(4)?;
        self.spec.encode(writer)?;
        writer.text(&self.term)?;
        self.size.encode(writer)?;
        writer.uint(self.lifetime_ms);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        let spec = ProcessSpec::decode(decoder)?;
        let term = decoder.text(MAX_TERM_LEN)?.to_owned();
        let size = TerminalSize::decode(decoder)?;
        Self::new(spec, (term, size), decoder.uint()?)
    }
}

/// Successful terminal summary of one Exec or Shell operation.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessResult {
    /// Observed exit status.
    pub termination: Termination,
    /// Why the operation ended.
    pub reason: EndReason,
    /// Wall-clock duration from spawn to exit.
    pub duration_ms: u64,
    /// Bytes of stdout (Exec) or terminal output (Shell) produced.
    pub output_bytes: u64,
    /// Bytes of stderr produced (always zero for Shell).
    pub error_bytes: u64,
}

/// Terminal Exec or Shell outcome.
pub type ProcessOutcome = Result<ProcessResult, ServiceError>;

impl ProcessResult {
    /// Creates a summary.
    pub const fn new(termination: Termination, reason: EndReason, duration_ms: u64) -> Self {
        Self {
            termination,
            reason,
            duration_ms,
            output_bytes: 0,
            error_bytes: 0,
        }
    }

    /// Returns this summary with output byte counts.
    #[must_use]
    pub const fn with_bytes(mut self, output_bytes: u64, error_bytes: u64) -> Self {
        self.output_bytes = output_bytes;
        self.error_bytes = error_bytes;
        self
    }

    /// Encodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INTERNAL`] only on allocation failure.
    pub fn encode_outcome(outcome: &ProcessOutcome) -> Result<Vec<u8>, ProtocolError> {
        cbor::encode_outcome(
            outcome.as_ref().map_err(|error| *error),
            |result, writer| {
                writer.array(5)?;
                result.termination.encode(writer)?;
                writer
                    .uint(result.reason.code())
                    .uint(result.duration_ms)
                    .uint(result.output_bytes)
                    .uint(result.error_bytes);
                Ok(())
            },
        )
    }

    /// Decodes one terminal outcome frame payload.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn decode_outcome(bytes: &[u8]) -> Result<ProcessOutcome, ProtocolError> {
        cbor::decode_outcome(bytes, |decoder| {
            decoder.array_exact(5)?;
            let termination = Termination::decode(decoder)?;
            let reason = EndReason::from_code(decoder.uint()?)?;
            let duration_ms = decoder.uint()?;
            Ok(Self::new(termination, reason, duration_ms)
                .with_bytes(decoder.uint()?, decoder.uint()?))
        })
    }
}
