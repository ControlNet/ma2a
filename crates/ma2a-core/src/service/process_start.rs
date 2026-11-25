//! The durable specification of one Process start.

use crate::{ProtocolError, space_codec::Decoder};

use super::{ProcessSpec, cbor::Writer};

/// Smallest per-Process retained output bound: 1 MiB.
pub const MIN_LOG_LIMIT: u32 = 1_048_576;
/// Largest per-Process retained output bound: 16 MiB.
pub const MAX_LOG_LIMIT: u32 = 16_777_216;
/// Retained output bound when a caller names none: 4 MiB.
pub const DEFAULT_LOG_LIMIT: u32 = 4_194_304;
/// Longest display name of a Process.
pub const MAX_PROCESS_NAME_LEN: usize = 64;
/// Most labels on one Process.
pub const MAX_PROCESS_LABELS: usize = 8;
/// Longest label key.
pub const MAX_LABEL_KEY_LEN: usize = 64;
/// Longest label value.
pub const MAX_LABEL_VALUE_LEN: usize = 256;

/// Program, stdin policy, output retention, and resource metadata of one Process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessStart {
    spec: ProcessSpec,
    stdin: bool,
    log_limit: u32,
    name: Option<String>,
    labels: Vec<(String, String)>,
}

impl ProcessStart {
    /// Validates a start; the program is required and `log_limit` must be in bounds.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] without a program or with an invalid limit.
    pub fn new(spec: ProcessSpec, stdin: bool, log_limit: u32) -> Result<Self, ProtocolError> {
        if spec.program().is_none() || !(MIN_LOG_LIMIT..=MAX_LOG_LIMIT).contains(&log_limit) {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            spec,
            stdin,
            log_limit,
            name: None,
            labels: Vec::new(),
        })
    }

    /// Adds a display name: 1–64 bytes without control characters.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an invalid name.
    pub fn with_name(mut self, name: String) -> Result<Self, ProtocolError> {
        if !valid_value(&name, MAX_PROCESS_NAME_LEN) {
            return Err(ProtocolError::INVALID_INPUT);
        }
        self.name = Some(name);
        Ok(self)
    }

    /// Adds one label; keys are `[a-z0-9][a-z0-9._-]*` and unique.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an invalid, duplicate, or excess label.
    pub fn with_label(mut self, key: String, value: String) -> Result<Self, ProtocolError> {
        if self.labels.len() >= MAX_PROCESS_LABELS
            || !valid_key(&key)
            || !(value.is_empty() || valid_value(&value, MAX_LABEL_VALUE_LEN))
            || self.labels.iter().any(|(existing, _)| *existing == key)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        self.labels.push((key, value));
        Ok(self)
    }

    /// Returns the program specification.
    pub const fn spec(&self) -> &ProcessSpec {
        &self.spec
    }

    /// Returns whether the Process keeps a stdin pipe for the single writer.
    pub const fn stdin(&self) -> bool {
        self.stdin
    }

    /// Returns the retained output bound in bytes.
    pub const fn log_limit(&self) -> u32 {
        self.log_limit
    }

    /// Returns the display name, if any.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Returns the labels in request order.
    pub fn labels(&self) -> &[(String, String)] {
        &self.labels
    }

    /// Encodes exact canonical bytes, as stored and fingerprinted.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the start exceeds a header frame.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        self.encode(&mut writer)?;
        writer.finish(super::MAX_HEADER_FRAME_LEN)
    }

    /// Decodes exact canonical bytes.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut decoder = Decoder::new(bytes);
        let start = Self::decode(&mut decoder)?;
        decoder.finish()?;
        Ok(start)
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer.array(5)?;
        self.spec.encode(writer)?;
        writer
            .boolean(self.stdin)
            .uint(u64::from(self.log_limit))
            .optional_text(self.name.as_deref())?
            .array(self.labels.len())?;
        for (key, value) in &self.labels {
            writer.array(2)?.text(key)?.text(value)?;
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(5)?;
        let spec = ProcessSpec::decode(decoder)?;
        let stdin = decoder.boolean()?;
        let log_limit = super::cbor::u32_value(decoder)?;
        let mut start = Self::new(spec, stdin, log_limit)?;
        if !decoder.null() {
            start = start.with_name(decoder.text(MAX_PROCESS_NAME_LEN)?.to_owned())?;
        }
        for _ in 0..decoder.array(MAX_PROCESS_LABELS)? {
            decoder.array_exact(2)?;
            let key = decoder.text(MAX_LABEL_KEY_LEN)?.to_owned();
            // A value may be empty; `with_label` refuses control characters.
            let value = decoder.text_without_nul(MAX_LABEL_VALUE_LEN)?.to_owned();
            start = start.with_label(key, value)?;
        }
        Ok(start)
    }
}

fn valid_value(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    key.len() <= MAX_LABEL_KEY_LEN
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}
