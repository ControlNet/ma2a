use crate::{ProtocolError, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceChain};

/// Maximum signed manifests returned in one enrollment page.
pub const MAX_ENROLLMENT_ARTIFACTS_PER_PAGE: usize = 8;
/// Maximum encoded enrollment page size.
pub const MAX_ENROLLMENT_PAGE_BYTES: usize = 300_000;

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
        Ok(pages)
    }

    /// Builds the explicitly invalid Genesis-plus-latest shape for rejection tests.
    ///
    /// # Errors
    ///
    /// Returns an error when the chain has no signed manifests.
    pub fn genesis_and_latest(chain: &SpaceChain) -> Result<Self, ProtocolError> {
        let latest = chain
            .manifests()
            .last()
            .ok_or(ProtocolError::INVALID_INPUT)?;
        Ok(Self {
            page_index: 0,
            page_count: 1,
            final_generation: chain.latest_generation(),
            genesis: Some(chain.genesis().canonical_bytes().to_vec()),
            manifests: vec![latest.canonical_bytes().to_vec()],
        })
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

/// Reconstructs only a complete, ordered, contiguous canonical Space chain.
///
/// # Errors
///
/// Returns an error unless every page and signed artifact forms one complete canonical chain.
pub fn validate_enrollment_pages(pages: &[EnrollmentPage]) -> Result<SpaceChain, ProtocolError> {
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
