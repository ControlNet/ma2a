use crate::{EndpointId, ProtocolError, SpaceId};

/// Maximum encoded bytes in one control request or response batch.
pub const MAX_CONTROL_BATCH_BYTES: usize = 1_048_576;
/// Maximum signed artifacts carried by one response page.
pub const MAX_CONTROL_ARTIFACTS_PER_PAGE: usize = 256;
/// Maximum Space cursors carried by one request.
pub const MAX_CONTROL_SPACES_PER_REQUEST: usize = 64;
/// Maximum address or relay high-water entries in one Space cursor.
pub const MAX_CONTROL_CURSOR_ENTRIES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ArtifactKind {
    Manifest,
    AddressRecord,
    RelayAdvertisement,
}

/// Closed signed-artifact kind carried by control synchronization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlArtifactKind(ArtifactKind);

impl ControlArtifactKind {
    /// Authority-signed contiguous Space manifest.
    pub const MANIFEST: Self = Self(ArtifactKind::Manifest);
    /// Endpoint-signed Space address record.
    pub const ADDRESS_RECORD: Self = Self(ArtifactKind::AddressRecord);
    /// Provider-signed Private Relay advertisement.
    pub const RELAY_ADVERTISEMENT: Self = Self(ArtifactKind::RelayAdvertisement);

    pub(crate) const fn code(self) -> u8 {
        match self.0 {
            ArtifactKind::Manifest => 0,
            ArtifactKind::AddressRecord => 1,
            ArtifactKind::RelayAdvertisement => 2,
        }
    }

    pub(crate) const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::MANIFEST),
            1 => Some(Self::ADDRESS_RECORD),
            2 => Some(Self::RELAY_ADVERTISEMENT),
            _ => None,
        }
    }
}

/// One subject-specific accepted sequence cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlCursorEntryV1 {
    endpoint_id: EndpointId,
    sequence: u64,
}

impl ControlCursorEntryV1 {
    /// Creates one Endpoint or provider high-water cursor.
    pub const fn new(endpoint_id: EndpointId, sequence: u64) -> Self {
        Self {
            endpoint_id,
            sequence,
        }
    }

    /// Returns the signed artifact subject.
    pub const fn endpoint_id(self) -> EndpointId {
        self.endpoint_id
    }

    /// Returns the highest accepted sequence.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }
}

/// Receiver high-water state for one locally known Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlCursorV1 {
    space_id: SpaceId,
    manifest_generation: u64,
    manifest_hash: [u8; 32],
    address_cursors: Vec<ControlCursorEntryV1>,
    relay_cursors: Vec<ControlCursorEntryV1>,
}

impl ControlCursorV1 {
    /// Creates one cursor without artifact sequence entries.
    pub const fn new(space_id: SpaceId, manifest_generation: u64, manifest_hash: [u8; 32]) -> Self {
        Self {
            space_id,
            manifest_generation,
            manifest_hash,
            address_cursors: Vec::new(),
            relay_cursors: Vec::new(),
        }
    }

    /// Attaches bounded canonical artifact sequence entries.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for excessive or duplicate subjects.
    pub fn with_entries(
        mut self,
        mut address_cursors: Vec<ControlCursorEntryV1>,
        mut relay_cursors: Vec<ControlCursorEntryV1>,
    ) -> Result<Self, ProtocolError> {
        canonicalize_entries(&mut address_cursors)?;
        canonicalize_entries(&mut relay_cursors)?;
        self.address_cursors = address_cursors;
        self.relay_cursors = relay_cursors;
        Ok(self)
    }

    /// Returns the non-authoritative Space cursor.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Returns the latest accepted manifest generation.
    pub const fn manifest_generation(&self) -> u64 {
        self.manifest_generation
    }

    /// Returns the latest accepted manifest chain hash.
    pub const fn manifest_hash(&self) -> [u8; 32] {
        self.manifest_hash
    }

    /// Returns canonical address-record cursors.
    pub fn address_cursors(&self) -> &[ControlCursorEntryV1] {
        &self.address_cursors
    }

    /// Returns canonical relay-advertisement cursors.
    pub fn relay_cursors(&self) -> &[ControlCursorEntryV1] {
        &self.relay_cursors
    }
}

/// Bounded control pull request across locally known Spaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlRequestV1 {
    cursors: Vec<ControlCursorV1>,
    push_pages: Vec<ControlPageV1>,
}

impl ControlRequestV1 {
    /// Creates a canonical request ordered by Space identifier.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for excessive or duplicate Spaces.
    pub fn new(mut cursors: Vec<ControlCursorV1>) -> Result<Self, ProtocolError> {
        if cursors.len() > MAX_CONTROL_SPACES_PER_REQUEST {
            return Err(ProtocolError::INVALID_INPUT);
        }
        cursors.sort_by_key(ControlCursorV1::space_id);
        if cursors
            .windows(2)
            .any(|pair| matches!(pair, [first, second] if first.space_id() == second.space_id()))
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            cursors,
            push_pages: Vec::new(),
        })
    }

    /// Returns canonical Space cursors.
    pub fn cursors(&self) -> &[ControlCursorV1] {
        &self.cursors
    }

    /// Attaches bounded opportunistic push pages for the same known Spaces.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when pages are excessive, non-canonical, duplicated,
    /// or not tied to a cursor Space.
    pub fn with_push_pages(
        mut self,
        push_pages: Vec<ControlPageV1>,
    ) -> Result<Self, ProtocolError> {
        validate_pages(&push_pages)?;
        if push_pages.iter().any(|page| {
            self.cursors
                .binary_search_by_key(&page.space_id(), ControlCursorV1::space_id)
                .is_err()
        }) {
            return Err(ProtocolError::INVALID_INPUT);
        }
        self.push_pages = push_pages;
        Ok(self)
    }

    /// Returns opportunistic Space-local push pages.
    pub fn push_pages(&self) -> &[ControlPageV1] {
        &self.push_pages
    }
}

/// Exact original signed artifact bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlArtifactV1 {
    kind: ControlArtifactKind,
    signed_bytes: Vec<u8>,
}

impl ControlArtifactV1 {
    /// Creates one bounded artifact without transforming signed bytes.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for empty or oversized signed bytes.
    pub fn new(kind: ControlArtifactKind, signed_bytes: Vec<u8>) -> Result<Self, ProtocolError> {
        if signed_bytes.is_empty() || signed_bytes.len() > MAX_CONTROL_BATCH_BYTES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self { kind, signed_bytes })
    }

    /// Returns the closed signed-artifact kind.
    pub const fn kind(&self) -> ControlArtifactKind {
        self.kind
    }

    /// Returns exact signed canonical bytes unchanged.
    pub fn signed_bytes(&self) -> &[u8] {
        &self.signed_bytes
    }
}

/// One bounded Space-local response page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlPageV1 {
    space_id: SpaceId,
    more: bool,
    artifacts: Vec<ControlArtifactV1>,
}

impl ControlPageV1 {
    /// Creates one page with at most 256 signed artifacts.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the artifact count exceeds 256.
    pub fn new(
        space_id: SpaceId,
        more: bool,
        artifacts: Vec<ControlArtifactV1>,
    ) -> Result<Self, ProtocolError> {
        if artifacts.len() > MAX_CONTROL_ARTIFACTS_PER_PAGE {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            space_id,
            more,
            artifacts,
        })
    }

    /// Returns the authorized Space represented by this page.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }

    /// Returns whether another bounded round may have additional artifacts.
    pub const fn has_more(&self) -> bool {
        self.more
    }

    /// Returns exact signed artifacts in validation order.
    pub fn artifacts(&self) -> &[ControlArtifactV1] {
        &self.artifacts
    }
}

/// Bounded response containing only authorized Space-local pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlResponseV1 {
    pages: Vec<ControlPageV1>,
}

impl ControlResponseV1 {
    /// Creates a response with at most one page per requested Space.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when pages are excessive, non-canonical, or duplicated.
    pub fn new(pages: Vec<ControlPageV1>) -> Result<Self, ProtocolError> {
        validate_pages(&pages)?;
        Ok(Self { pages })
    }

    /// Returns authorized Space-local pages.
    pub fn pages(&self) -> &[ControlPageV1] {
        &self.pages
    }
}

fn canonicalize_entries(entries: &mut [ControlCursorEntryV1]) -> Result<(), ProtocolError> {
    if entries.len() > MAX_CONTROL_CURSOR_ENTRIES {
        return Err(ProtocolError::INVALID_INPUT);
    }
    entries.sort_by_key(|entry| entry.endpoint_id());
    if entries
        .windows(2)
        .any(|pair| matches!(pair, [first, second] if first.endpoint_id() == second.endpoint_id()))
    {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(())
}

fn validate_pages(pages: &[ControlPageV1]) -> Result<(), ProtocolError> {
    if pages.len() > MAX_CONTROL_SPACES_PER_REQUEST
        || pages
            .windows(2)
            .any(|pair| matches!(pair, [first, second] if first.space_id() >= second.space_id()))
    {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(())
}

mod control_codec;
