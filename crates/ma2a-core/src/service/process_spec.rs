//! Typed program specification shared by Exec and Shell.

use crate::{ProtocolError, space_codec::Decoder};

use super::cbor::Writer;

/// Maximum byte length of a program, working directory, argument, or environment value.
pub const MAX_PROCESS_TEXT_LEN: usize = 4_096;
/// Maximum number of arguments after the program.
pub const MAX_PROCESS_ARGS: usize = 256;
/// Maximum number of environment additions.
pub const MAX_PROCESS_ENV: usize = 64;
/// Maximum byte length of an environment variable name.
pub const MAX_ENV_NAME_LEN: usize = 128;

/// Program, typed argv, working directory, and bounded environment additions.
///
/// There is no shell-string form. `program` is a bare name resolved through the target
/// Runtime's `PATH`, or an absolute path; `cwd`, when present, must be absolute on the target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSpec {
    program: Option<String>,
    args: Vec<String>,
    cwd: Option<String>,
    env: Vec<(String, String)>,
}

impl ProcessSpec {
    /// Validates a program specification. `program` may be `None` only for Shell.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for out-of-bound or invalid fields.
    pub fn new(
        program: Option<String>,
        args: Vec<String>,
        cwd: Option<String>,
    ) -> Result<Self, ProtocolError> {
        let valid_program = program
            .as_deref()
            .is_none_or(|value| valid_text(value) && !value.is_empty());
        let valid_cwd = cwd
            .as_deref()
            .is_none_or(|value| valid_text(value) && !value.is_empty());
        if !valid_program
            || !valid_cwd
            || args.len() > MAX_PROCESS_ARGS
            || !args.iter().all(|argument| valid_text(argument))
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            program,
            args,
            cwd,
            env: Vec::new(),
        })
    }

    /// Adds one environment variable, rejecting invalid names and duplicate keys.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for invalid, duplicate, or excess entries.
    pub fn with_env(mut self, name: String, value: String) -> Result<Self, ProtocolError> {
        if self.env.len() >= MAX_PROCESS_ENV
            || !valid_env_name(&name)
            || !valid_text(&value)
            || self.env.iter().any(|(existing, _)| *existing == name)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        self.env.push((name, value));
        Ok(self)
    }

    /// Returns the program, or `None` for the target's default shell.
    pub fn program(&self) -> Option<&str> {
        self.program.as_deref()
    }

    /// Returns the typed arguments after the program.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Returns the absolute target working directory, if any.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Returns the environment additions in request order.
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        writer
            .array(4)?
            .optional_text(self.program.as_deref())?
            .array(self.args.len())?;
        for argument in &self.args {
            writer.text(argument)?;
        }
        writer
            .optional_text(self.cwd.as_deref())?
            .array(self.env.len())?;
        for (name, value) in &self.env {
            writer.array(2)?.text(name)?.text(value)?;
        }
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(4)?;
        let program = optional(decoder)?;
        let count = decoder.array(MAX_PROCESS_ARGS)?;
        let args = (0..count)
            .map(|_| {
                decoder
                    .text_without_nul(MAX_PROCESS_TEXT_LEN)
                    .map(str::to_owned)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cwd = optional(decoder)?;
        let mut spec = Self::new(program, args, cwd)?;
        let count = decoder.array(MAX_PROCESS_ENV)?;
        for _ in 0..count {
            decoder.array_exact(2)?;
            let name = decoder.text(MAX_ENV_NAME_LEN)?.to_owned();
            let value = decoder.text_without_nul(MAX_PROCESS_TEXT_LEN)?.to_owned();
            spec = spec.with_env(name, value)?;
        }
        Ok(spec)
    }
}

fn optional(decoder: &mut Decoder<'_>) -> Result<Option<String>, ProtocolError> {
    if decoder.null() {
        Ok(None)
    } else {
        decoder
            .text_without_nul(MAX_PROCESS_TEXT_LEN)
            .map(|value| Some(value.to_owned()))
    }
}

fn valid_text(value: &str) -> bool {
    value.len() <= MAX_PROCESS_TEXT_LEN && !value.contains('\0')
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= MAX_ENV_NAME_LEN
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// How a child process ended, as the target operating system reported it.
#[expect(
    clippy::exhaustive_enums,
    reason = "termination forms are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    /// The child exited with this status code.
    Exited(u32),
    /// The child was ended by this signal (Unix).
    Signaled(u32),
    /// No exit status could be observed.
    Unknown,
}

impl Termination {
    pub(crate) fn encode(self, writer: &mut Writer) -> Result<(), ProtocolError> {
        match self {
            Self::Exited(code) => writer.array(2)?.uint(0).uint(u64::from(code)),
            Self::Signaled(signal) => writer.array(2)?.uint(1).uint(u64::from(signal)),
            Self::Unknown => writer.array(1)?.uint(2),
        };
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        let length = decoder.array(2)?;
        match (length, decoder.uint()?) {
            (2, 0) => Ok(Self::Exited(super::cbor::u32_value(decoder)?)),
            (2, 1) => Ok(Self::Signaled(super::cbor::u32_value(decoder)?)),
            (1, 2) => Ok(Self::Unknown),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}

/// Why an Exec, Shell, or Proxy operation ended.
#[expect(
    clippy::exhaustive_enums,
    reason = "termination reasons are a closed wire vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The child exited on its own, or a proxy stream closed normally.
    Completed,
    /// The requested timeout or session lifetime expired and the child was killed.
    TimedOut,
    /// The client cancelled or disconnected and the child was killed.
    Cancelled,
    /// Runtime shutdown killed the child.
    Shutdown,
}

impl EndReason {
    /// Returns the stable lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Shutdown => "shutdown",
        }
    }

    pub(crate) const fn code(self) -> u64 {
        match self {
            Self::Completed => 0,
            Self::TimedOut => 1,
            Self::Cancelled => 2,
            Self::Shutdown => 3,
        }
    }

    pub(crate) const fn from_code(code: u64) -> Result<Self, ProtocolError> {
        match code {
            0 => Ok(Self::Completed),
            1 => Ok(Self::TimedOut),
            2 => Ok(Self::Cancelled),
            3 => Ok(Self::Shutdown),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }
}
