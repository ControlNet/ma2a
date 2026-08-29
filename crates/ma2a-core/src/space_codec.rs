use crate::ProtocolError;

pub(crate) const MAX_SPACE_OBJECT_LEN: usize = 32_768;

pub(crate) fn buffer() -> Result<Vec<u8>, ProtocolError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve(MAX_SPACE_OBJECT_LEN)
        .map_err(|_| ProtocolError::INTERNAL)?;
    Ok(bytes)
}

pub(crate) fn write_uint(output: &mut Vec<u8>, value: u64) {
    write_major(output, 0, value);
}

pub(crate) fn write_map(output: &mut Vec<u8>, length: usize) -> Result<(), ProtocolError> {
    write_length(output, 5, length)
}

pub(crate) fn write_array(output: &mut Vec<u8>, length: usize) -> Result<(), ProtocolError> {
    write_length(output, 4, length)
}

pub(crate) fn write_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ProtocolError> {
    write_length(output, 2, bytes.len())?;
    output.extend_from_slice(bytes);
    Ok(())
}

pub(crate) fn write_text(output: &mut Vec<u8>, text: &str) -> Result<(), ProtocolError> {
    write_length(output, 3, text.len())?;
    output.extend_from_slice(text.as_bytes());
    Ok(())
}

pub(crate) fn write_bool(output: &mut Vec<u8>, value: bool) {
    output.push(if value { 0xf5 } else { 0xf4 });
}

fn write_length(output: &mut Vec<u8>, major: u8, length: usize) -> Result<(), ProtocolError> {
    let value = u64::try_from(length).map_err(|_| ProtocolError::INVALID_INPUT)?;
    write_major(output, major, value);
    Ok(())
}

fn write_major(output: &mut Vec<u8>, major: u8, value: u64) {
    let prefix = major << 5;
    let [
        byte_0,
        byte_1,
        byte_2,
        byte_3,
        byte_4,
        byte_5,
        byte_6,
        byte_7,
    ] = value.to_be_bytes();
    match value {
        0..=23 => output.push(prefix | byte_7),
        24..=255 => output.extend_from_slice(&[prefix | 24, byte_7]),
        256..=65_535 => {
            output.push(prefix | 25);
            output.extend_from_slice(&[byte_6, byte_7]);
        }
        65_536..=4_294_967_295 => {
            output.push(prefix | 26);
            output.extend_from_slice(&[byte_4, byte_5, byte_6, byte_7]);
        }
        _ => {
            output.push(prefix | 27);
            output.extend_from_slice(&[
                byte_0, byte_1, byte_2, byte_3, byte_4, byte_5, byte_6, byte_7,
            ]);
        }
    }
}

pub(crate) struct Decoder<'a> {
    input: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) const fn new(input: &'a [u8]) -> Self {
        Self { input, offset: 0 }
    }

    pub(crate) fn map(&mut self, expected: usize) -> Result<(), ProtocolError> {
        if self.length(5)? == expected {
            Ok(())
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    pub(crate) fn array(&mut self, maximum: usize) -> Result<usize, ProtocolError> {
        let length = self.length(4)?;
        if length <= maximum {
            Ok(length)
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    pub(crate) fn key(&mut self, expected: u64) -> Result<(), ProtocolError> {
        if self.uint()? == expected {
            Ok(())
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    pub(crate) fn uint(&mut self) -> Result<u64, ProtocolError> {
        self.major(0)
    }

    pub(crate) fn bytes(&mut self, maximum: usize) -> Result<&'a [u8], ProtocolError> {
        let length = self.length(2)?;
        if length > maximum {
            return Err(ProtocolError::INVALID_INPUT);
        }
        self.take(length)
    }

    pub(crate) fn text(&mut self, maximum: usize) -> Result<&'a str, ProtocolError> {
        let length = self.length(3)?;
        if length == 0 || length > maximum {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let text =
            std::str::from_utf8(self.take(length)?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        if text.chars().any(char::is_control) {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(text)
    }

    pub(crate) fn boolean(&mut self) -> Result<bool, ProtocolError> {
        match self.byte()? {
            0xf4 => Ok(false),
            0xf5 => Ok(true),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }

    pub(crate) const fn finish(self) -> Result<(), ProtocolError> {
        if self.offset == self.input.len() {
            Ok(())
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    fn length(&mut self, major: u8) -> Result<usize, ProtocolError> {
        usize::try_from(self.major(major)?).map_err(|_| ProtocolError::INVALID_INPUT)
    }

    fn major(&mut self, expected: u8) -> Result<u64, ProtocolError> {
        let header = self.byte()?;
        if header >> 5 != expected {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let additional = header & 0x1f;
        match additional {
            value @ 0..=23 => Ok(u64::from(value)),
            24 => self.extended(1, 24),
            25 => self.extended(2, 256),
            26 => self.extended(4, 65_536),
            27 => self.extended(8, u64::from(u32::MAX) + 1),
            _ => Err(ProtocolError::INVALID_INPUT),
        }
    }

    fn extended(&mut self, width: usize, minimum: u64) -> Result<u64, ProtocolError> {
        let bytes = self.take(width)?;
        let mut value = 0u64;
        for byte in bytes {
            value = (value << 8) | u64::from(*byte);
        }
        if value >= minimum {
            Ok(value)
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }

    fn byte(&mut self) -> Result<u8, ProtocolError> {
        let value = *self
            .input
            .get(self.offset)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        self.offset += 1;
        Ok(value)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        let value = self
            .input
            .get(self.offset..end)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        self.offset = end;
        Ok(value)
    }
}
