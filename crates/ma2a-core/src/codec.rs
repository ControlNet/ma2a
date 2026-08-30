//! Canonical CBOR encoding and strict bounded decoding for Phase 1 envelopes.

use crate::wire::ResponseKind;
use crate::{
    EndpointId, MAX_PAYLOAD_LEN, MAX_WIRE_LEN, ProtocolError, ProtocolVersion, RequestEnvelope,
    RequestId, RequestOperation, ResponseEnvelope, ResponseResult, ServiceKind,
};

const REQUEST_FIELDS: u8 = 0xa4;
const RESPONSE_FIELDS: u8 = 0xa3;
const PAIR: u8 = 0x82;

/// Encodes a request into its exact canonical CBOR bytes.
///
/// # Errors
/// Returns `ProtocolError::INTERNAL` if bounded output allocation fails.
pub fn encode_request(request: &RequestEnvelope<'_>) -> Result<Vec<u8>, ProtocolError> {
    let mut output = canonical_buffer()?;
    output.push(REQUEST_FIELDS);
    write_version(&mut output, request.version());
    output.extend_from_slice(&[1, 0x50]);
    output.extend_from_slice(request.request_id().as_bytes());
    output.extend_from_slice(&[2, 0x58, 0x20]);
    output.extend_from_slice(request.target().as_bytes());
    output.extend_from_slice(&[3, PAIR, 0]);
    match request.operation().service_kind() {
        ServiceKind::Echo => {}
    }
    write_bytes(&mut output, request.operation().payload())?;
    Ok(output)
}

/// Decodes only exact canonical request bytes and borrows their payload.
///
/// # Errors
/// Returns `ProtocolError::VERSION_MISMATCH` for any version other than 1.0, otherwise
/// `ProtocolError::INVALID_INPUT` for noncanonical, malformed, trailing, or oversized input.
pub fn decode_request(bytes: &[u8]) -> Result<RequestEnvelope<'_>, ProtocolError> {
    if bytes.len() > MAX_WIRE_LEN {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let mut decoder = Decoder::new(bytes);
    decoder.consume(REQUEST_FIELDS)?;
    decoder.consume(0)?;
    decoder.read_version()?;
    decoder.consume(1)?;
    let request_id = RequestId::try_from(decoder.read_bytes(16)?)?;
    decoder.consume(2)?;
    let target = EndpointId::try_from(decoder.read_bytes(32)?)?;
    decoder.consume(3)?;
    decoder.consume(PAIR)?;
    decoder.consume(0)?;
    let operation = RequestOperation::echo(decoder.read_bytes(MAX_PAYLOAD_LEN)?)?;
    decoder.finish()?;
    Ok(RequestEnvelope::from_decoded(request_id, target, operation))
}

/// Encodes a response into its exact canonical CBOR bytes.
///
/// # Errors
/// Returns `ProtocolError::INTERNAL` if bounded output allocation fails.
pub fn encode_response(response: &ResponseEnvelope<'_>) -> Result<Vec<u8>, ProtocolError> {
    let mut output = canonical_buffer()?;
    output.push(RESPONSE_FIELDS);
    write_version(&mut output, response.version());
    output.extend_from_slice(&[1, 0x50]);
    output.extend_from_slice(response.request_id().as_bytes());
    output.extend_from_slice(&[2, PAIR]);
    match response.result().kind() {
        ResponseKind::Echo(payload) => {
            output.push(0);
            write_bytes(&mut output, payload)?;
        }
        ResponseKind::Error(error) => output.extend_from_slice(&[1, error.code()]),
    }
    Ok(output)
}

/// Decodes only exact canonical response bytes and borrows their payload.
///
/// # Errors
/// Returns `ProtocolError::VERSION_MISMATCH` for any version other than 1.0, otherwise
/// `ProtocolError::INVALID_INPUT` for noncanonical, malformed, trailing, or oversized input.
pub fn decode_response(bytes: &[u8]) -> Result<ResponseEnvelope<'_>, ProtocolError> {
    if bytes.len() > MAX_WIRE_LEN {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let mut decoder = Decoder::new(bytes);
    decoder.consume(RESPONSE_FIELDS)?;
    decoder.consume(0)?;
    decoder.read_version()?;
    decoder.consume(1)?;
    let request_id = RequestId::try_from(decoder.read_bytes(16)?)?;
    decoder.consume(2)?;
    decoder.consume(PAIR)?;
    let result = match decoder.read_byte()? {
        0 => ResponseResult::from_kind(ResponseKind::Echo(
            decoder.read_bytes(MAX_PAYLOAD_LEN)?.into(),
        )),
        1 => ResponseResult::from_kind(ResponseKind::Error(
            ProtocolError::from_code(decoder.read_small_uint()?)
                .ok_or(ProtocolError::INVALID_INPUT)?,
        )),
        _ => return Err(ProtocolError::INVALID_INPUT),
    };
    decoder.finish()?;
    Ok(ResponseEnvelope::from_decoded(request_id, result))
}

fn canonical_buffer() -> Result<Vec<u8>, ProtocolError> {
    let mut output = Vec::new();
    output
        .try_reserve(MAX_WIRE_LEN)
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(output)
}

fn write_version(output: &mut Vec<u8>, version: ProtocolVersion) {
    output.extend_from_slice(&[0, PAIR, version.major(), version.minor()]);
}

fn write_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ProtocolError> {
    match bytes.len() {
        length @ 0..=23 => {
            let length = u8::try_from(length).map_err(|_| ProtocolError::INTERNAL)?;
            output.push(0x40 | length);
        }
        length @ 24..=255 => {
            let length = u8::try_from(length).map_err(|_| ProtocolError::INTERNAL)?;
            output.extend_from_slice(&[0x58, length]);
        }
        length @ 256..=MAX_PAYLOAD_LEN => {
            let length = u16::try_from(length).map_err(|_| ProtocolError::INTERNAL)?;
            output.push(0x59);
            output.extend_from_slice(&length.to_be_bytes());
        }
        _ => return Err(ProtocolError::INVALID_INPUT),
    }
    output.extend_from_slice(bytes);
    Ok(())
}

struct Decoder<'a> {
    remaining: &'a [u8],
}

impl<'a> Decoder<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn consume(&mut self, expected: u8) -> Result<(), ProtocolError> {
        if self.read_byte()? == expected {
            Ok(())
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    fn read_version(&mut self) -> Result<(), ProtocolError> {
        self.consume(PAIR)?;
        let version = ProtocolVersion::from_parts(self.read_u8()?, self.read_u8()?);
        if version == ProtocolVersion::V1 {
            Ok(())
        } else {
            Err(ProtocolError::VERSION_MISMATCH)
        }
    }

    fn read_small_uint(&mut self) -> Result<u8, ProtocolError> {
        let value = self.read_byte()?;
        if value <= 23 {
            Ok(value)
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        match self.read_byte()? {
            value @ 0..=23 => Ok(value),
            0x18 => {
                let value = self.read_byte()?;
                if value >= 24 {
                    Ok(value)
                } else {
                    Err(ProtocolError::INVALID_INPUT)
                }
            }
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }

    fn read_bytes(&mut self, maximum: usize) -> Result<&'a [u8], ProtocolError> {
        let header = self.read_byte()?;
        let length = match header {
            0x40..=0x57 => usize::from(header & 0x1f),
            0x58 => {
                let length = usize::from(self.read_byte()?);
                if length < 24 {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                length
            }
            0x59 => {
                let bytes = self.take(2)?;
                let bytes =
                    <&[u8; 2]>::try_from(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
                let length = usize::from(u16::from_be_bytes(*bytes));
                if length < 256 {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                length
            }
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        if length > maximum {
            return Err(ProtocolError::INVALID_INPUT);
        }
        self.take(length)
    }

    fn read_byte(&mut self) -> Result<u8, ProtocolError> {
        let (byte, remaining) = self
            .remaining
            .split_first()
            .ok_or(ProtocolError::INVALID_INPUT)?;
        self.remaining = remaining;
        Ok(*byte)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let (value, remaining) = self
            .remaining
            .split_at_checked(length)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        self.remaining = remaining;
        Ok(value)
    }

    const fn finish(self) -> Result<(), ProtocolError> {
        if self.remaining.is_empty() {
            Ok(())
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }
}
