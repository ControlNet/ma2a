//! The payloads a hosted service puts in its `accept` and `outcome` frames.
//!
//! The Runtime relays both verbatim; only their shape is fixed, so any caller can read any
//! hosted service: `accept` is one CBOR byte string, and a successful `outcome` result is one
//! CBOR byte string. What the bytes mean is the service's own versioned contract.

use crate::{ProtocolError, space_codec::Decoder};

use super::{ServiceError, cbor};

/// Longest `accept` header a hosted service may send.
pub const MAX_CALL_ACCEPT_LEN: usize = 8_192;
/// Longest successful result a hosted service may send in its `outcome`.
pub const MAX_CALL_RESULT_LEN: usize = 60_000;

/// Encodes one hosted `accept` header.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] when `header` exceeds [`MAX_CALL_ACCEPT_LEN`].
pub fn encode_call_accept(header: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    if header.len() > MAX_CALL_ACCEPT_LEN {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let mut writer = cbor::Writer::new()?;
    writer.bytes(header)?;
    writer.finish(super::MAX_HEADER_FRAME_LEN)
}

/// Decodes one hosted `accept` header.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
pub fn decode_call_accept(bytes: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let mut decoder = Decoder::new(bytes);
    let header = decoder.bytes(MAX_CALL_ACCEPT_LEN)?.to_vec();
    decoder.finish()?;
    Ok(header)
}

/// Encodes one hosted `outcome`: the result bytes, or the failure.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] when the result exceeds [`MAX_CALL_RESULT_LEN`].
pub fn encode_call_outcome(outcome: Result<&[u8], ServiceError>) -> Result<Vec<u8>, ProtocolError> {
    cbor::encode_outcome(
        outcome.as_ref().map_err(|error| *error),
        |result, writer| {
            if result.len() > MAX_CALL_RESULT_LEN {
                return Err(ProtocolError::INVALID_INPUT);
            }
            writer.bytes(result)?;
            Ok(())
        },
    )
}

/// Decodes one hosted `outcome`.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] for malformed or noncanonical input.
pub fn decode_call_outcome(bytes: &[u8]) -> Result<Result<Vec<u8>, ServiceError>, ProtocolError> {
    cbor::decode_outcome(bytes, |decoder| {
        Ok(decoder.bytes(MAX_CALL_RESULT_LEN)?.to_vec())
    })
}
