use ma2a_core::{EndpointId, ProtocolError, RequestId, SpaceId};
use serde_json::{Map, Value};

use super::{ApiError, commands::BoundedText};

pub(crate) fn exact_fields(object: &Map<String, Value>, required: &[&str]) -> Result<(), ApiError> {
    if object.len() != required.len() || object.keys().any(|key| !required.contains(&key.as_str()))
    {
        Err(ApiError::invalid_input())
    } else {
        Ok(())
    }
}

pub(crate) fn text<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, ApiError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(ApiError::invalid_input)
}

pub(crate) fn bounded_text(
    object: &Map<String, Value>,
    field: &str,
    maximum: usize,
) -> Result<BoundedText, ApiError> {
    BoundedText::parse(text(object, field)?, maximum)
}

pub(crate) fn number(object: &Map<String, Value>, field: &str) -> Result<u64, ApiError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(ApiError::invalid_input)
}

pub(crate) fn request_id(object: &Map<String, Value>, field: &str) -> Result<RequestId, ApiError> {
    let bytes = decode_hex::<16>(text(object, field)?)?;
    RequestId::try_from(bytes.as_slice()).map_err(|_| ApiError::invalid_input())
}

pub(crate) fn endpoint_id(
    object: &Map<String, Value>,
    field: &str,
) -> Result<EndpointId, ApiError> {
    let bytes = decode_hex::<32>(text(object, field)?)?;
    EndpointId::try_from(bytes.as_slice()).map_err(|_| ApiError::invalid_input())
}

pub(crate) fn space_id(object: &Map<String, Value>, field: &str) -> Result<SpaceId, ApiError> {
    let bytes = decode_hex::<32>(text(object, field)?)?;
    SpaceId::try_from(bytes.as_slice()).map_err(|_| ApiError::invalid_input())
}

pub(crate) fn space_ids(
    object: &Map<String, Value>,
    field: &str,
) -> Result<Vec<SpaceId>, ApiError> {
    object
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(ApiError::invalid_input)?
        .iter()
        .map(|value| {
            let bytes = decode_hex::<32>(value.as_str().ok_or_else(ApiError::invalid_input)?)?;
            SpaceId::try_from(bytes.as_slice()).map_err(|_| ApiError::invalid_input())
        })
        .collect()
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(hex_character(byte >> 4));
        output.push(hex_character(byte & 0x0f));
    }
    output
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], ApiError> {
    if value.len() != N * 2 {
        return Err(ApiError::invalid_input());
    }
    let mut output = [0_u8; N];
    for (slot, pair) in output.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let high = hex_nibble(pair.first().copied())?;
        let low = hex_nibble(pair.get(1).copied())?;
        *slot = (high << 4) | low;
    }
    Ok(output)
}

const fn hex_nibble(value: Option<u8>) -> Result<u8, ApiError> {
    match value {
        Some(byte @ b'0'..=b'9') => Ok(byte - b'0'),
        Some(byte @ b'a'..=b'f') => Ok(byte - b'a' + 10),
        Some(_) | None => Err(ApiError::new(ProtocolError::INVALID_INPUT)),
    }
}

const fn hex_character(value: u8) -> char {
    match value {
        0 => '0',
        1 => '1',
        2 => '2',
        3 => '3',
        4 => '4',
        5 => '5',
        6 => '6',
        7 => '7',
        8 => '8',
        9 => '9',
        10 => 'a',
        11 => 'b',
        12 => 'c',
        13 => 'd',
        14 => 'e',
        _ => 'f',
    }
}
