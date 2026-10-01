//! The local API messages this SDK sends and reads, as JSON.

use ma2a_runtime::api::{self, Command};
use serde_json::{Map, Value};

use crate::Error;

/// Decodes one local API command written as JSON fields; `version` is added here.
pub(crate) fn command(mut fields: Map<String, Value>) -> Result<Command, Error> {
    fields.insert("version".to_owned(), Value::from(api::LOCAL_API_VERSION));
    let bytes = serde_json::to_vec(&Value::Object(fields))
        .map_err(|_| Error::Protocol("a local API command could not be encoded"))?;
    api::decode_command(&bytes).map_err(|_| Error::Protocol("an argument exceeds its bound"))
}

/// Reads one local API response: its result payload, or the refusal it carries.
pub(crate) fn payload(response: &[u8], expected: &str) -> Result<Map<String, Value>, Error> {
    let mut document: Value = serde_json::from_slice(response)
        .map_err(|_| Error::Protocol("the Runtime answered malformed JSON"))?;
    if let Some(error) = document.get("error").and_then(Value::as_str) {
        let remediation = document
            .get("remediation")
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Err(Error::Refused {
            error: error.to_owned(),
            remediation,
        });
    }
    if document.pointer("/result/type").and_then(Value::as_str) != Some(expected) {
        return Err(Error::Protocol("the Runtime answered an unexpected result"));
    }
    match document.pointer_mut("/result/payload").map(Value::take) {
        Some(Value::Object(payload)) => Ok(payload),
        _ => Err(Error::Protocol("the Runtime answered an unexpected result")),
    }
}

/// Returns lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _written = write!(text, "{byte:02x}");
            text
        })
}

/// Parses exactly `N` bytes of lowercase hex.
pub(crate) fn unhex<const N: usize>(text: &str) -> Result<[u8; N], Error> {
    let malformed = Error::Protocol("malformed hex");
    let digits = text.as_bytes();
    if digits.len() != N * 2 {
        return Err(malformed);
    }
    let mut bytes = [0_u8; N];
    for (byte, pair) in bytes.iter_mut().zip(digits.chunks_exact(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| Error::Protocol("malformed hex"))?;
        if pair.bytes().any(|digit| digit.is_ascii_uppercase()) {
            return Err(Error::Protocol("malformed hex"));
        }
        *byte = u8::from_str_radix(pair, 16).map_err(|_| Error::Protocol("malformed hex"))?;
    }
    Ok(bytes)
}

/// Returns a fresh random request ID.
pub(crate) fn request_id() -> Result<ma2a_core::RequestId, Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| Error::Protocol("no randomness for a request ID"))?;
    ma2a_core::RequestId::try_from(bytes.as_slice())
        .map_err(|_| Error::Protocol("no randomness for a request ID"))
}
