use crate::{ProtocolError, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceChain};

/// Maximum signed manifests returned in one enrollment page.
pub const MAX_ENROLLMENT_ARTIFACTS_PER_PAGE: usize = 8;
/// Maximum encoded enrollment page size.
pub const MAX_ENROLLMENT_PAGE_BYTES: usize = 300_000;
/// Maximum pages in one complete enrollment response.
pub const MAX_ENROLLMENT_PAGES: usize = 32;
const ENROLLMENT_PAGE_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
/// One ordered bounded page of signed chain artifacts.
pub struct EnrollmentPage {
    page_index: u16,
    page_count: u16,
    final_generation: u64,
    genesis: Option<Vec<u8>>,
    manifests: Vec<Vec<u8>>,
}

impl EnrollmentPage {
    /// Splits a complete chain into bounded ordered pages.
    ///
    /// # Errors
    ///
    /// Returns an error when the page count exceeds the protocol's bounded representation.
    pub fn paginate(chain: &SpaceChain) -> Result<Vec<Self>, ProtocolError> {
        let count = chain
            .manifests()
            .len()
            .div_ceil(MAX_ENROLLMENT_ARTIFACTS_PER_PAGE)
            .max(1);
        if count > MAX_ENROLLMENT_PAGES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let page_count = u16::try_from(count).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let mut pages = Vec::with_capacity(usize::from(page_count));
        for page_index in 0..page_count {
            let start = usize::from(page_index) * MAX_ENROLLMENT_ARTIFACTS_PER_PAGE;
            let end = (start + MAX_ENROLLMENT_ARTIFACTS_PER_PAGE).min(chain.manifests().len());
            let manifests = chain
                .manifests()
                .get(start..end)
                .ok_or(ProtocolError::INVALID_INPUT)?
                .iter()
                .map(|manifest| manifest.canonical_bytes().to_vec())
                .collect();
            pages.push(Self {
                page_index,
                page_count,
                final_generation: chain.latest_generation(),
                genesis: (page_index == 0).then(|| chain.genesis().canonical_bytes().to_vec()),
                manifests,
            });
        }
        for page in &pages {
            let _encoded = page.encode()?;
        }
        Ok(pages)
    }

    /// Encodes one bounded transport page.
    ///
    /// # Errors
    /// Returns an error when a field or the complete page exceeds protocol bounds.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        if self.manifests.len() > MAX_ENROLLMENT_ARTIFACTS_PER_PAGE {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let genesis_len = self.genesis.as_ref().map_or(0, Vec::len);
        let mut output = Vec::new();
        output.push(ENROLLMENT_PAGE_VERSION);
        output.extend_from_slice(&self.page_index.to_be_bytes());
        output.extend_from_slice(&self.page_count.to_be_bytes());
        output.extend_from_slice(&self.final_generation.to_be_bytes());
        write_bytes(&mut output, self.genesis.as_deref().unwrap_or_default())?;
        output.extend_from_slice(
            &u16::try_from(self.manifests.len())
                .map_err(|_| ProtocolError::INVALID_INPUT)?
                .to_be_bytes(),
        );
        for manifest in &self.manifests {
            write_bytes(&mut output, manifest)?;
        }
        if genesis_len == 0 && self.page_index == 0 || output.len() > MAX_ENROLLMENT_PAGE_BYTES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(output)
    }

    /// Decodes one bounded untrusted transport page.
    ///
    /// # Errors
    /// Returns an error for malformed, oversized, or non-canonical framing.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_ENROLLMENT_PAGE_BYTES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut cursor = 0;
        if take::<1>(bytes, &mut cursor)?[0] != ENROLLMENT_PAGE_VERSION {
            return Err(ProtocolError::VERSION_MISMATCH);
        }
        let page_index = u16::from_be_bytes(take::<2>(bytes, &mut cursor)?);
        let page_count = u16::from_be_bytes(take::<2>(bytes, &mut cursor)?);
        if page_count == 0 || usize::from(page_count) > MAX_ENROLLMENT_PAGES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let final_generation = u64::from_be_bytes(take::<8>(bytes, &mut cursor)?);
        let genesis = read_bytes(bytes, &mut cursor)?;
        let count = usize::from(u16::from_be_bytes(take::<2>(bytes, &mut cursor)?));
        if count > MAX_ENROLLMENT_ARTIFACTS_PER_PAGE {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut manifests = Vec::with_capacity(count);
        for _ in 0..count {
            manifests.push(read_bytes(bytes, &mut cursor)?.ok_or(ProtocolError::INVALID_INPUT)?);
        }
        if cursor != bytes.len() {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let page = Self {
            page_index,
            page_count,
            final_generation,
            genesis,
            manifests,
        };
        if page.encode()? != bytes {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(page)
    }

    /// Returns the zero-based page index.
    pub const fn page_index(&self) -> u16 {
        self.page_index
    }
    /// Returns the total page count.
    pub const fn page_count(&self) -> u16 {
        self.page_count
    }
    /// Returns the generation reached after the final page.
    pub const fn final_generation(&self) -> u64 {
        self.final_generation
    }
    /// Returns Genesis bytes only for the first page.
    pub fn genesis(&self) -> Option<&[u8]> {
        self.genesis.as_deref()
    }
    /// Returns the ordered canonical manifest bytes in this page.
    pub fn manifests(&self) -> &[Vec<u8>] {
        &self.manifests
    }
}

fn write_bytes(output: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ProtocolError> {
    output.extend_from_slice(
        &u32::try_from(bytes.len())
            .map_err(|_| ProtocolError::INVALID_INPUT)?
            .to_be_bytes(),
    );
    output.extend_from_slice(bytes);
    Ok(())
}

fn read_bytes(bytes: &[u8], cursor: &mut usize) -> Result<Option<Vec<u8>>, ProtocolError> {
    let length = usize::try_from(u32::from_be_bytes(take::<4>(bytes, cursor)?))
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    if length == 0 {
        return Ok(None);
    }
    let end = cursor
        .checked_add(length)
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(ProtocolError::INVALID_INPUT)?
        .to_vec();
    *cursor = end;
    Ok(Some(value))
}

fn take<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], ProtocolError> {
    let end = cursor.checked_add(N).ok_or(ProtocolError::INVALID_INPUT)?;
    let value = <[u8; N]>::try_from(
        bytes
            .get(*cursor..end)
            .ok_or(ProtocolError::INVALID_INPUT)?,
    )
    .map_err(|_| ProtocolError::INVALID_INPUT)?;
    *cursor = end;
    Ok(value)
}

/// Reconstructs only a complete, ordered, contiguous canonical Space chain.
///
/// # Errors
///
/// Returns an error unless every page and signed artifact forms one complete canonical chain.
pub fn validate_enrollment_pages(pages: &[EnrollmentPage]) -> Result<SpaceChain, ProtocolError> {
    if pages.len() > MAX_ENROLLMENT_PAGES {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let first = pages.first().ok_or(ProtocolError::INVALID_INPUT)?;
    if usize::from(first.page_count) != pages.len() || first.page_index != 0 {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let genesis_bytes = first
        .genesis
        .as_deref()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let genesis = SignedSpaceGenesisV1::from_canonical_bytes(genesis_bytes)?;
    let mut chain = SpaceChain::from_genesis(genesis).map_err(|_| ProtocolError::INVALID_INPUT)?;
    for (index, page) in pages.iter().enumerate() {
        if usize::from(page.page_index) != index
            || page.page_count != first.page_count
            || page.final_generation != first.final_generation
            || (index > 0 && page.genesis.is_some())
            || page.manifests.len() > MAX_ENROLLMENT_ARTIFACTS_PER_PAGE
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        for bytes in &page.manifests {
            let manifest =
                SignedSpaceManifestV1::from_canonical_bytes(bytes, chain.genesis().authority())?;
            chain
                .apply(&manifest)
                .map_err(|_| ProtocolError::INVALID_INPUT)?;
        }
    }
    if chain.latest_generation() != first.final_generation {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(chain)
}
