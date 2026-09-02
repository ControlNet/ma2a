use crate::{
    EnrollmentPage, MAX_ADDRESS_RECORD_LEN, MAX_ENROLLMENT_PAGE_BYTES, MAX_ENROLLMENT_PAGES,
    ProtocolError, SignedSpaceAddressRecordV1, SpaceChain, validate_enrollment_pages,
};

const ENROLLMENT_BOOTSTRAP_MAGIC: [u8; 4] = *b"MEB1";
const FRAME_HEADER_BYTES: usize = 12;

/// Maximum encoded enrollment bootstrap frame size.
pub const MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES: usize =
    FRAME_HEADER_BYTES + MAX_ENROLLMENT_PAGE_BYTES + MAX_ADDRESS_RECORD_LEN;

/// A complete enrolled chain paired with the owner's signed reachability record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnrollmentBootstrap {
    chain: SpaceChain,
    owner_address_record: SignedSpaceAddressRecordV1,
}

impl EnrollmentBootstrap {
    /// Creates a bootstrap from already verified signed artifacts.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when the owner record is scoped to another Space.
    pub fn new(
        chain: SpaceChain,
        owner_address_record: SignedSpaceAddressRecordV1,
    ) -> Result<Self, ProtocolError> {
        if owner_address_record.record().space_id() != chain.space_id() {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            chain,
            owner_address_record,
        })
    }

    /// Encodes the bootstrap into canonical bounded enrollment response frames.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] when pagination or a frame exceeds protocol bounds.
    pub fn encode_frames(&self) -> Result<Vec<Vec<u8>>, ProtocolError> {
        EnrollmentPage::paginate(&self.chain)?
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let page = page.encode()?;
                let address = if index == 0 {
                    self.owner_address_record.canonical_bytes()
                } else {
                    &[]
                };
                let mut frame = Vec::with_capacity(FRAME_HEADER_BYTES + page.len() + address.len());
                frame.extend_from_slice(&ENROLLMENT_BOOTSTRAP_MAGIC);
                write_length(&mut frame, page.len())?;
                frame.extend_from_slice(&page);
                write_length(&mut frame, address.len())?;
                frame.extend_from_slice(address);
                if frame.len() > MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                Ok(frame)
            })
            .collect()
    }

    /// Decodes exact canonical bootstrap frames and reconstructs the signed chain.
    ///
    /// # Errors
    /// Returns [`ProtocolError`] for malformed, noncanonical, incomplete, or oversized input.
    pub fn decode_frames(frames: &[Vec<u8>]) -> Result<Self, ProtocolError> {
        if frames.is_empty() || frames.len() > MAX_ENROLLMENT_PAGES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut pages = Vec::with_capacity(frames.len());
        let mut owner_address_record = None;
        for (index, frame) in frames.iter().enumerate() {
            let mut reader = FrameReader::new(frame)?;
            let page_length = reader.length(MAX_ENROLLMENT_PAGE_BYTES)?;
            if page_length == 0 {
                return Err(ProtocolError::INVALID_INPUT);
            }
            pages.push(EnrollmentPage::decode(reader.take(page_length)?)?);
            let address_length = reader.length(MAX_ADDRESS_RECORD_LEN)?;
            match (index, address_length) {
                (0, 0) => return Err(ProtocolError::INVALID_INPUT),
                (0, length) => {
                    owner_address_record = Some(SignedSpaceAddressRecordV1::parse_canonical_bytes(
                        reader.take(length)?,
                    )?);
                }
                (_, 0) => {}
                (_, _) => return Err(ProtocolError::INVALID_INPUT),
            }
            reader.finish()?;
        }
        let bootstrap = Self::new(
            validate_enrollment_pages(&pages)?,
            owner_address_record.ok_or(ProtocolError::INVALID_INPUT)?,
        )?;
        if bootstrap.encode_frames()?.as_slice() != frames {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(bootstrap)
    }

    /// Returns the complete verified enrolled chain.
    pub const fn chain(&self) -> &SpaceChain {
        &self.chain
    }

    /// Returns the exact canonical owner-signed address record.
    pub const fn owner_address_record(&self) -> &SignedSpaceAddressRecordV1 {
        &self.owner_address_record
    }

    /// Consumes the bootstrap into its verified signed artifacts.
    pub fn into_parts(self) -> (SpaceChain, SignedSpaceAddressRecordV1) {
        (self.chain, self.owner_address_record)
    }
}

fn write_length(output: &mut Vec<u8>, length: usize) -> Result<(), ProtocolError> {
    output.extend_from_slice(
        &u32::try_from(length)
            .map_err(|_| ProtocolError::INVALID_INPUT)?
            .to_be_bytes(),
    );
    Ok(())
}

struct FrameReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> FrameReader<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES
            || !bytes.starts_with(&ENROLLMENT_BOOTSTRAP_MAGIC)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { bytes, offset: 4 })
    }

    fn length(&mut self, maximum: usize) -> Result<usize, ProtocolError> {
        let length = usize::try_from(u32::from_be_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| ProtocolError::INVALID_INPUT)?,
        ))
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
        if length > maximum {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(length)
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

    const fn finish(self) -> Result<(), ProtocolError> {
        if self.offset != self.bytes.len() {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(())
    }
}
