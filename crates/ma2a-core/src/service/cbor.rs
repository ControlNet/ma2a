//! Canonical CBOR helpers for bounded service objects.

use crate::{
    ProtocolError,
    space_codec::{
        Decoder, small_buffer, write_array, write_bool, write_bytes, write_map, write_null,
        write_text, write_uint,
    },
};

use super::{RemotePath, RootName, ServiceError};

/// Canonical writer for one bounded service object.
pub(crate) struct Writer(Vec<u8>);

impl Writer {
    pub(crate) fn new() -> Result<Self, ProtocolError> {
        small_buffer(256).map(Self)
    }

    pub(crate) fn map(&mut self, length: usize) -> Result<&mut Self, ProtocolError> {
        write_map(&mut self.0, length)?;
        Ok(self)
    }

    pub(crate) fn array(&mut self, length: usize) -> Result<&mut Self, ProtocolError> {
        write_array(&mut self.0, length)?;
        Ok(self)
    }

    pub(crate) fn uint(&mut self, value: u64) -> &mut Self {
        write_uint(&mut self.0, value);
        self
    }

    pub(crate) fn boolean(&mut self, value: bool) -> &mut Self {
        write_bool(&mut self.0, value);
        self
    }

    pub(crate) fn text(&mut self, value: &str) -> Result<&mut Self, ProtocolError> {
        write_text(&mut self.0, value)?;
        Ok(self)
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> Result<&mut Self, ProtocolError> {
        write_bytes(&mut self.0, value)?;
        Ok(self)
    }

    pub(crate) fn null(&mut self) -> &mut Self {
        write_null(&mut self.0);
        self
    }

    pub(crate) fn optional_text(
        &mut self,
        value: Option<&str>,
    ) -> Result<&mut Self, ProtocolError> {
        match value {
            Some(text) => self.text(text),
            None => Ok(self.null()),
        }
    }

    pub(crate) fn optional_uint(&mut self, value: Option<u64>) -> &mut Self {
        match value {
            Some(number) => self.uint(number),
            None => self.null(),
        }
    }

    pub(crate) fn texts(&mut self, values: &[String]) -> Result<&mut Self, ProtocolError> {
        self.array(values.len())?;
        for value in values {
            self.text(value)?;
        }
        Ok(self)
    }

    pub(crate) fn path(&mut self, path: &RemotePath) -> Result<&mut Self, ProtocolError> {
        self.array(2)?
            .text(path.root().as_str())?
            .texts(path.components())
    }

    /// Returns the object after checking it fits its frame bound.
    pub(crate) fn finish(self, maximum: usize) -> Result<Vec<u8>, ProtocolError> {
        if self.0.len() <= maximum {
            Ok(self.0)
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }
}

/// Reads an optional text value that must be non-empty when present.
pub(crate) fn optional_text(
    decoder: &mut Decoder<'_>,
    maximum: usize,
) -> Result<Option<String>, ProtocolError> {
    if decoder.null() {
        Ok(None)
    } else {
        decoder.text(maximum).map(|text| Some(text.to_owned()))
    }
}

pub(crate) fn optional_uint(decoder: &mut Decoder<'_>) -> Result<Option<u64>, ProtocolError> {
    if decoder.null() {
        Ok(None)
    } else {
        decoder.uint().map(Some)
    }
}

pub(crate) fn texts(
    decoder: &mut Decoder<'_>,
    bounds: (usize, usize),
) -> Result<Vec<String>, ProtocolError> {
    let (items, bytes) = bounds;
    let length = decoder.array(items)?;
    (0..length)
        .map(|_| decoder.text(bytes).map(str::to_owned))
        .collect()
}

pub(crate) fn path(decoder: &mut Decoder<'_>) -> Result<RemotePath, ProtocolError> {
    decoder.array_exact(2)?;
    let root = RootName::new(decoder.text(super::MAX_ROOT_NAME_LEN)?)?;
    let components = texts(
        decoder,
        (super::MAX_PATH_COMPONENTS, super::MAX_PATH_COMPONENT_LEN),
    )?;
    RemotePath::new(root, components)
}

pub(crate) fn u16_value(decoder: &mut Decoder<'_>) -> Result<u16, ProtocolError> {
    u16::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)
}

pub(crate) fn u32_value(decoder: &mut Decoder<'_>) -> Result<u32, ProtocolError> {
    u32::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)
}

pub(crate) fn hash(decoder: &mut Decoder<'_>) -> Result<[u8; 32], ProtocolError> {
    <[u8; 32]>::try_from(decoder.bytes(32)?).map_err(|_| ProtocolError::INVALID_INPUT)
}

/// Encodes `{0: 0, 1: result}` on success or `{0: code, 1: null}` on failure.
pub(crate) fn encode_outcome<T>(
    outcome: Result<&T, ServiceError>,
    encode: impl FnOnce(&T, &mut Writer) -> Result<(), ProtocolError>,
) -> Result<Vec<u8>, ProtocolError> {
    let mut writer = Writer::new()?;
    writer.map(2)?.uint(0);
    match outcome {
        Ok(value) => {
            writer.uint(0).uint(1);
            encode(value, &mut writer)?;
        }
        Err(error) => {
            writer.uint(u64::from(error.code())).uint(1).null();
        }
    }
    writer.finish(super::MAX_OUTCOME_FRAME_LEN)
}

const MAX_CBOR_SKIP_DEPTH: usize = 16;

/// Decodes the envelope written by [`encode_outcome`].
pub(crate) fn decode_outcome<T>(
    bytes: &[u8],
    decode: impl FnOnce(&mut Decoder<'_>) -> Result<T, ProtocolError>,
) -> Result<Result<T, ServiceError>, ProtocolError> {
    let mut decoder = Decoder::new(bytes);
    let status = decode_outcome_header(&mut decoder)?;
    let outcome = if status == 0 {
        Ok(decode(&mut decoder)?)
    } else {
        Err(decode_outcome_error(&mut decoder, status)?)
    };
    decoder.finish()?;
    Ok(outcome)
}

/// Reads only the status of an outcome envelope: `None` for success.
pub(crate) fn outcome_status(bytes: &[u8]) -> Result<Option<ServiceError>, ProtocolError> {
    let mut decoder = Decoder::new(bytes);
    let status = decode_outcome_header(&mut decoder)?;
    let outcome = if status == 0 {
        skip_value(&mut decoder, MAX_CBOR_SKIP_DEPTH)?;
        None
    } else {
        Some(decode_outcome_error(&mut decoder, status)?)
    };
    decoder.finish()?;
    Ok(outcome)
}

fn decode_outcome_header(decoder: &mut Decoder<'_>) -> Result<u8, ProtocolError> {
    decoder.map(2)?;
    decoder.key(0)?;
    let status = u8::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
    decoder.key(1)?;
    Ok(status)
}

fn decode_outcome_error(
    decoder: &mut Decoder<'_>,
    status: u8,
) -> Result<ServiceError, ProtocolError> {
    if !decoder.null() {
        return Err(ProtocolError::INVALID_INPUT);
    }
    ServiceError::from_code(status).ok_or(ProtocolError::INVALID_INPUT)
}

fn skip_value(decoder: &mut Decoder<'_>, depth: usize) -> Result<(), ProtocolError> {
    if depth == 0 {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let header = decoder.peek()?;
    match header >> 5 {
        0 => {
            decoder.uint()?;
        }
        2 => {
            decoder.bytes(usize::MAX)?;
        }
        3 => {
            decoder.utf8_text(usize::MAX)?;
        }
        4 => {
            let length = decoder.length(4)?;
            for _ in 0..length {
                skip_value(decoder, depth - 1)?;
            }
        }
        5 => {
            let length = decoder.length(5)?;
            for _ in 0..length {
                skip_value(decoder, depth - 1)?;
                skip_value(decoder, depth - 1)?;
            }
        }
        7 => match header {
            0xf4 | 0xf5 => {
                decoder.boolean()?;
            }
            0xf6 => {
                if !decoder.null() {
                    return Err(ProtocolError::INVALID_INPUT);
                }
            }
            _ => return Err(ProtocolError::INVALID_INPUT),
        },
        _ => return Err(ProtocolError::INVALID_INPUT),
    }
    Ok(())
}
