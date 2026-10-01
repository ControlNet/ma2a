//! Names of hosted services and of their operations.
//!
//! A [`ServiceName`] names one service a third-party program serves through its Runtime. Like
//! an Agent name it routes and is consented to per Space; it never authenticates.

use core::fmt;

use crate::{ProtocolError, ServiceKind, space_codec::Decoder};

/// Longest hosted service name in bytes.
pub const MAX_SERVICE_NAME_LEN: usize = 63;
/// Longest operation name in bytes.
pub const MAX_OPERATION_NAME_LEN: usize = 32;

/// The operator-chosen name of one hosted service, unique within its Runtime.
///
/// 1–63 bytes of `[a-z0-9-]`, starting with a letter and not ending with `-`. Built-in family
/// names, `service` included, are reserved.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceName(String);

impl ServiceName {
    /// Validates one hosted service name.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for any name outside the closed form, and for
    /// a reserved name.
    pub fn new(name: &str) -> Result<Self, ProtocolError> {
        let bytes = name.as_bytes();
        let valid = !bytes.is_empty()
            && bytes.len() <= MAX_SERVICE_NAME_LEN
            && bytes.first().is_some_and(u8::is_ascii_lowercase)
            && bytes.last() != Some(&b'-')
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
            && ServiceKind::from_name(name).is_none();
        if valid {
            Ok(Self(name.to_owned()))
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Returns the name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Self::new(decoder.text(MAX_SERVICE_NAME_LEN)?)
    }
}

impl fmt::Display for ServiceName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The name of one operation of a service version.
///
/// 1–32 bytes of `[a-z0-9_.-]`, starting with a letter.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationName(String);

impl OperationName {
    /// Validates one operation name.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for any name outside the closed form.
    pub fn new(name: &str) -> Result<Self, ProtocolError> {
        let bytes = name.as_bytes();
        let valid = !bytes.is_empty()
            && bytes.len() <= MAX_OPERATION_NAME_LEN
            && bytes.first().is_some_and(u8::is_ascii_lowercase)
            && bytes.iter().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(byte)
            });
        if valid {
            Ok(Self(name.to_owned()))
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    /// Returns the name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Self::new(decoder.text(MAX_OPERATION_NAME_LEN)?)
    }
}

impl fmt::Display for OperationName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{OperationName, ServiceName};

    #[test]
    fn service_names_follow_the_closed_form_and_spare_built_in_names() {
        for valid in ["example", "a", "kv-store2", &"a".repeat(63)] {
            assert!(ServiceName::new(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "",
            "Example",
            "9lives",
            "trailing-",
            "has_underscore",
            "files",
            "agent",
            "service",
            "echo",
            &"a".repeat(64),
        ] {
            assert!(ServiceName::new(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn operation_names_follow_the_closed_form() {
        for valid in ["echo", "process_start", "v2.count", "a-b", &"a".repeat(32)] {
            assert!(OperationName::new(valid).is_ok(), "{valid}");
        }
        for invalid in ["", "Echo", "_x", "1x", "a b", "a/b", &"a".repeat(33)] {
            assert!(OperationName::new(invalid).is_err(), "{invalid}");
        }
    }
}
