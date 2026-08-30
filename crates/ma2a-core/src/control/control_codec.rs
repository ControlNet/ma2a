use crate::{EndpointId, ProtocolError, SpaceId};

use super::{
    ControlArtifactKind, ControlArtifactV1, ControlCursorEntryV1, ControlCursorV1, ControlPageV1,
    ControlRequestV1, ControlResponseV1, MAX_CONTROL_BATCH_BYTES, MAX_CONTROL_CURSOR_ENTRIES,
    MAX_CONTROL_SPACES_PER_REQUEST,
};

const REQUEST_MAGIC: [u8; 4] = *b"MCQ1";
const RESPONSE_MAGIC: [u8; 4] = *b"MCR1";

impl ControlRequestV1 {
    /// Encodes the canonical bounded request.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when the request exceeds protocol bounds.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = Vec::new();
        output.extend_from_slice(&REQUEST_MAGIC);
        write_count(&mut output, self.cursors().len())?;
        for cursor in self.cursors() {
            output.extend_from_slice(cursor.space_id().as_bytes());
            output.extend_from_slice(&cursor.manifest_generation().to_be_bytes());
            output.extend_from_slice(&cursor.manifest_hash());
            write_entries(&mut output, cursor.address_cursors())?;
            write_entries(&mut output, cursor.relay_cursors())?;
        }
        write_pages(&mut output, self.push_pages())?;
        bound(output)
    }

    /// Decodes one exact canonical bounded request.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when bytes are invalid, non-canonical, or exceed protocol bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(bytes, REQUEST_MAGIC)?;
        let count = reader.count(MAX_CONTROL_SPACES_PER_REQUEST)?;
        let mut cursors = Vec::with_capacity(count);
        for _index in 0..count {
            let space_id = SpaceId::try_from(reader.take(32)?)?;
            let generation = reader.u64()?;
            let manifest_hash = reader.array()?;
            let addresses = reader.entries()?;
            let relays = reader.entries()?;
            cursors.push(
                ControlCursorV1::new(space_id, generation, manifest_hash)
                    .with_entries(addresses, relays)?,
            );
        }
        let push_pages = reader.pages()?;
        reader.finish()?;
        let request = Self::new(cursors)?.with_push_pages(push_pages)?;
        if request.encode()?.as_slice() != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(request)
    }
}

impl ControlResponseV1 {
    /// Encodes exact signed artifact bytes into one bounded response.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when the response exceeds protocol bounds.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut output = Vec::new();
        output.extend_from_slice(&RESPONSE_MAGIC);
        write_pages(&mut output, self.pages())?;
        bound(output)
    }

    /// Decodes one exact canonical bounded response.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when bytes are invalid, non-canonical, or exceed protocol bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let mut reader = Reader::new(bytes, RESPONSE_MAGIC)?;
        let pages = reader.pages()?;
        reader.finish()?;
        let response = Self::new(pages)?;
        if response.encode()?.as_slice() != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(response)
    }
}

fn write_pages(output: &mut Vec<u8>, pages: &[ControlPageV1]) -> Result<(), ProtocolError> {
    write_count(output, pages.len())?;
    for page in pages {
        output.extend_from_slice(page.space_id().as_bytes());
        output.push(u8::from(page.has_more()));
        write_count(output, page.artifacts().len())?;
        for artifact in page.artifacts() {
            output.push(artifact.kind().code());
            let length = u32::try_from(artifact.signed_bytes().len())
                .map_err(|_| ProtocolError::INVALID_INPUT)?;
            output.extend_from_slice(&length.to_be_bytes());
            output.extend_from_slice(artifact.signed_bytes());
        }
    }
    Ok(())
}

fn write_entries(
    output: &mut Vec<u8>,
    entries: &[ControlCursorEntryV1],
) -> Result<(), ProtocolError> {
    write_count(output, entries.len())?;
    for entry in entries {
        output.extend_from_slice(entry.endpoint_id().as_bytes());
        output.extend_from_slice(&entry.sequence().to_be_bytes());
    }
    Ok(())
}

fn write_count(output: &mut Vec<u8>, count: usize) -> Result<(), ProtocolError> {
    output.extend_from_slice(
        &u16::try_from(count)
            .map_err(|_| ProtocolError::INVALID_INPUT)?
            .to_be_bytes(),
    );
    Ok(())
}

fn bound(output: Vec<u8>) -> Result<Vec<u8>, ProtocolError> {
    if output.len() > MAX_CONTROL_BATCH_BYTES {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(output)
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], magic: [u8; 4]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_CONTROL_BATCH_BYTES || !bytes.starts_with(&magic) {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { bytes, offset: 4 })
    }

    fn byte(&mut self) -> Result<u8, ProtocolError> {
        let byte = *self
            .bytes
            .get(self.offset)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        Ok(byte)
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ProtocolError::INVALID_INPUT)?;
        self.offset = end;
        Ok(value)
    }

    fn count(&mut self, maximum: usize) -> Result<usize, ProtocolError> {
        let count = usize::from(u16::from_be_bytes(self.array()?));
        if count > maximum {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(count)
    }

    fn u32(&mut self) -> Result<u32, ProtocolError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, ProtocolError> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], ProtocolError> {
        self.take(N)?
            .try_into()
            .map_err(|_| ProtocolError::INVALID_INPUT)
    }

    fn entries(&mut self) -> Result<Vec<ControlCursorEntryV1>, ProtocolError> {
        let count = self.count(MAX_CONTROL_CURSOR_ENTRIES)?;
        let mut entries = Vec::with_capacity(count);
        for _index in 0..count {
            entries.push(ControlCursorEntryV1::new(
                EndpointId::try_from(self.take(32)?)?,
                self.u64()?,
            ));
        }
        Ok(entries)
    }

    fn pages(&mut self) -> Result<Vec<ControlPageV1>, ProtocolError> {
        let count = self.count(MAX_CONTROL_SPACES_PER_REQUEST)?;
        let mut pages = Vec::with_capacity(count);
        for _index in 0..count {
            let space_id = SpaceId::try_from(self.take(32)?)?;
            let more = match self.byte()? {
                0 => false,
                1 => true,
                _ => return Err(ProtocolError::INVALID_INPUT),
            };
            let artifact_count = self.count(super::MAX_CONTROL_ARTIFACTS_PER_PAGE)?;
            let mut artifacts = Vec::with_capacity(artifact_count);
            for _artifact_index in 0..artifact_count {
                let kind = ControlArtifactKind::from_code(self.byte()?)
                    .ok_or(ProtocolError::INVALID_INPUT)?;
                let length =
                    usize::try_from(self.u32()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
                artifacts.push(ControlArtifactV1::new(kind, self.take(length)?.to_vec())?);
            }
            pages.push(ControlPageV1::new(space_id, more, artifacts)?);
        }
        Ok(pages)
    }

    const fn finish(self) -> Result<(), ProtocolError> {
        if self.offset != self.bytes.len() {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(())
    }
}
