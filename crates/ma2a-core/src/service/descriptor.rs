//! What a service offers: versions, operations, and how each operation's frames flow.
//!
//! A [`ServiceManifest`] is what an operator registers for a hosted service; a
//! [`ServiceDescriptor`] is what discovery returns for any service, built-in or hosted. Neither
//! ever names a Space, a member, or an authorization path.

use crate::{ProtocolError, ServiceKind, space_codec::Decoder};

use super::{ServiceName, ServiceVersion, cbor::Writer};

/// Longest service description in bytes.
pub const MAX_SERVICE_DESCRIPTION_LEN: usize = 256;
/// Most versions one service declares.
pub const MAX_SERVICE_VERSIONS: usize = 8;
/// Most operations one version declares.
pub const MAX_VERSION_OPERATIONS: usize = 16;
/// Most operations one service declares across all of its versions.
pub const MAX_SERVICE_OPERATIONS: usize = 64;
/// Most concurrent calls a hosted service may accept.
pub const MAX_SERVICE_CALLS: u8 = 64;
/// Longest canonical manifest encoding in bytes.
pub const MAX_MANIFEST_LEN: usize = 4_096;

/// Validates a version list: 1–8 versions, strictly ascending, at most 64 operations in all.
fn check_versions(versions: &[ServiceVersion]) -> Result<(), ProtocolError> {
    let ascending = versions
        .windows(2)
        .all(|pair| matches!(pair, [low, high] if low.version() < high.version()));
    let operations: usize = versions
        .iter()
        .map(|version| version.operations().len())
        .sum();
    if versions.is_empty()
        || versions.len() > MAX_SERVICE_VERSIONS
        || !ascending
        || operations > MAX_SERVICE_OPERATIONS
    {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(())
}

fn check_description(description: &str) -> Result<(), ProtocolError> {
    if description.len() > MAX_SERVICE_DESCRIPTION_LEN || description.contains('\0') {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(())
}

fn encode_versions(writer: &mut Writer, versions: &[ServiceVersion]) -> Result<(), ProtocolError> {
    writer.array(versions.len())?;
    versions
        .iter()
        .try_for_each(|version| version.encode(writer))
}

fn decode_versions(decoder: &mut Decoder<'_>) -> Result<Vec<ServiceVersion>, ProtocolError> {
    let count = decoder.array(MAX_SERVICE_VERSIONS)?;
    (0..count)
        .map(|_| ServiceVersion::decode(decoder))
        .collect()
}

/// What an operator registers for one hosted service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceManifest {
    description: String,
    versions: Vec<ServiceVersion>,
    max_calls: u8,
}

impl ServiceManifest {
    /// Validates one manifest.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for a description over 256 bytes or with a NUL,
    /// a version list outside its bounds, or `max_calls` outside 1–64.
    pub fn new(
        description: &str,
        versions: Vec<ServiceVersion>,
        max_calls: u8,
    ) -> Result<Self, ProtocolError> {
        check_description(description)?;
        check_versions(&versions)?;
        if !(1..=MAX_SERVICE_CALLS).contains(&max_calls) {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            description: description.to_owned(),
            versions,
            max_calls,
        })
    }

    /// Returns the description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the versions, ascending.
    pub fn versions(&self) -> &[ServiceVersion] {
        &self.versions
    }

    /// Returns one version, if declared.
    pub fn version(&self, version: u16) -> Option<&ServiceVersion> {
        self.versions.iter().find(|item| item.version() == version)
    }

    /// Returns the most calls served at once.
    pub const fn max_calls(&self) -> u8 {
        self.max_calls
    }

    /// Encodes the canonical bytes `[description, max_calls, versions]`.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when the encoding exceeds its bound.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let mut writer = Writer::new()?;
        writer
            .array(3)?
            .text(&self.description)?
            .uint(u64::from(self.max_calls));
        encode_versions(&mut writer, &self.versions)?;
        writer.finish(MAX_MANIFEST_LEN)
    }

    /// Decodes exact canonical bytes.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for malformed, noncanonical, or out-of-bound
    /// input.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_MANIFEST_LEN {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut decoder = Decoder::new(bytes);
        decoder.array_exact(3)?;
        let description = decoder
            .text_without_nul(MAX_SERVICE_DESCRIPTION_LEN)?
            .to_owned();
        let max_calls = u8::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let versions = decode_versions(&mut decoder)?;
        decoder.finish()?;
        Self::new(&description, versions, max_calls)
    }
}

/// Which kind of service a descriptor describes.
#[expect(
    clippy::exhaustive_enums,
    reason = "a service is either built in or hosted"
)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ServiceOrigin {
    /// A built-in family of the Runtime.
    Builtin(ServiceKind),
    /// A service a third-party program serves through the Runtime.
    Hosted(ServiceName),
}

impl ServiceOrigin {
    /// Returns the name discovery shows.
    pub fn name(&self) -> &str {
        match self {
            Self::Builtin(kind) => kind.name(),
            Self::Hosted(name) => name.as_str(),
        }
    }
}

/// What discovery returns for one service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceDescriptor {
    origin: ServiceOrigin,
    description: String,
    versions: Vec<ServiceVersion>,
    available: bool,
}

impl ServiceDescriptor {
    /// Validates one descriptor.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for a description or version list outside its
    /// bounds.
    pub fn new(
        origin: ServiceOrigin,
        description: &str,
        (versions, available): (Vec<ServiceVersion>, bool),
    ) -> Result<Self, ProtocolError> {
        check_description(description)?;
        check_versions(&versions)?;
        Ok(Self {
            origin,
            description: description.to_owned(),
            versions,
            available,
        })
    }

    /// Describes one hosted service from its manifest.
    pub fn hosted(name: ServiceName, manifest: &ServiceManifest, available: bool) -> Self {
        Self {
            origin: ServiceOrigin::Hosted(name),
            description: manifest.description.clone(),
            versions: manifest.versions.clone(),
            available,
        }
    }

    /// Returns what kind of service this is.
    pub const fn origin(&self) -> &ServiceOrigin {
        &self.origin
    }

    /// Returns the description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the versions, ascending.
    pub fn versions(&self) -> &[ServiceVersion] {
        &self.versions
    }

    /// Returns whether the service takes calls now.
    pub const fn available(&self) -> bool {
        self.available
    }

    pub(crate) fn encode(&self, writer: &mut Writer) -> Result<(), ProtocolError> {
        let origin = match self.origin {
            ServiceOrigin::Builtin(_) => 0,
            ServiceOrigin::Hosted(_) => 1,
        };
        writer
            .array(5)?
            .uint(origin)
            .text(self.origin.name())?
            .text(&self.description)?;
        encode_versions(writer, &self.versions)?;
        writer.boolean(self.available);
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.array_exact(5)?;
        let origin = match decoder.uint()? {
            0 => ServiceKind::from_name(decoder.text(super::MAX_SERVICE_NAME_LEN)?)
                .map(ServiceOrigin::Builtin)
                .ok_or(ProtocolError::INVALID_INPUT)?,
            1 => ServiceOrigin::Hosted(ServiceName::decode_from(decoder)?),
            _ => return Err(ProtocolError::INVALID_INPUT),
        };
        let description = decoder
            .text_without_nul(MAX_SERVICE_DESCRIPTION_LEN)?
            .to_owned();
        let versions = decode_versions(decoder)?;
        let available = decoder.boolean()?;
        Self::new(origin, &description, (versions, available))
    }
}

/// Returns the highest version both sides speak, so selection is deterministic.
pub fn select_version(supported: &[u16], descriptor: &ServiceDescriptor) -> Option<u16> {
    descriptor
        .versions
        .iter()
        .map(ServiceVersion::version)
        .filter(|version| supported.contains(version))
        .max()
}
