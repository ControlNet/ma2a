use crate::space_codec::{
    Decoder, write_array, write_bool, write_bytes, write_map, write_text, write_uint,
};
use crate::{EndpointId, ProtocolError};

/// Maximum number of members or revocations accepted in one Space object.
pub const MAX_SPACE_MEMBERS: usize = 64;
/// Maximum UTF-8 byte length of a Space member label.
pub const MAX_MEMBER_LABEL_LEN: usize = 64;

/// Returns the transitional member label for one Endpoint.
///
/// A member label names an Endpoint inside a Space and is deliberately not the
/// Space name: the two are separate signed facts and reusing one as the other
/// makes a Space look like a member. Phase 1 has no user-facing Endpoint naming,
/// so the label is derived from the Endpoint's own identity and is therefore
/// stable, unique, and never confusable with a Space name.
pub fn default_member_label(endpoint_id: EndpointId) -> String {
    use std::fmt::Write as _;
    let mut label = String::with_capacity(25);
    label.push_str("endpoint-");
    for byte in endpoint_id.as_bytes().iter().take(8) {
        let _written = write!(&mut label, "{byte:02x}");
    }
    label
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// A capability that a Space policy and member can grant.
pub struct Capability(u16);

impl Capability {
    /// Echo-service access.
    pub const ECHO: Self = Self(1);
    /// Permission to provide private relay service.
    pub const PRIVATE_RELAY_PROVIDER: Self = Self(2);
    /// Caller permission to stat, list, read, and download from exported file roots.
    pub const FILES_READ: Self = Self(4);
    /// Caller permission to create, write, rename, remove, and upload into read-write roots.
    pub const FILES_WRITE: Self = Self(8);
    /// Caller permission to run one-shot Exec on a member that exposes it.
    pub const EXEC: Self = Self(16);
    /// Caller permission to open an interactive Shell on a member that exposes it.
    pub const SHELL: Self = Self(32);
    /// Caller permission to open TCP Proxy streams to destinations a member allows.
    pub const PROXY: Self = Self(64);

    /// Returns whether the genesis Space policy, rather than member grants alone, governs this.
    ///
    /// Remote-service capabilities are granted only by the authority-signed member record. The
    /// Phase 1 genesis policy is fixed at creation, so extending it would leave every earlier
    /// Space unable ever to adopt them without adding authority beyond the owner's signature.
    pub const fn is_policy_governed(self) -> bool {
        matches!(self.0, 1 | 2)
    }
}

/// Every member capability bit accepted by this protocol version.
const KNOWN_CAPABILITY_BITS: u16 = 0x7f;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Capability grants attached to one Space member.
pub struct MemberCapabilities(u16);

impl MemberCapabilities {
    /// Creates capability grants from the supported Phase 1 flags.
    pub const fn new(echo: bool, private_relay_provider: bool) -> Self {
        let echo_bit = if echo { 1 } else { 0 };
        let relay_bit = if private_relay_provider { 2 } else { 0 };
        Self(echo_bit | relay_bit)
    }

    /// Returns these grants with `capability` added.
    #[must_use]
    pub const fn with(self, capability: Capability) -> Self {
        Self(self.0 | capability.0)
    }

    /// Returns these grants with `capability` removed.
    #[must_use]
    pub const fn without(self, capability: Capability) -> Self {
        Self(self.0 & !capability.0)
    }

    /// Returns whether these grants include `capability`.
    pub const fn allows(self, capability: Capability) -> bool {
        self.0 & capability.0 != 0
    }

    pub(crate) const fn bits(self) -> u16 {
        self.0
    }

    pub(crate) const fn from_bits(bits: u16) -> Result<Self, ProtocolError> {
        if bits & !KNOWN_CAPABILITY_BITS == 0 {
            Ok(Self(bits))
        } else {
            Err(ProtocolError::INVALID_INPUT)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Version 1 Space-wide capability and membership limits.
pub struct SpacePolicyV1 {
    echo: bool,
    private_relay_provider: bool,
    maximum_members: u8,
}

impl SpacePolicyV1 {
    /// Returns the Phase 1 default policy.
    pub const fn phase_one_default() -> Self {
        Self {
            echo: true,
            private_relay_provider: true,
            maximum_members: 64,
        }
    }

    /// Returns whether this policy enables `capability`.
    pub const fn allows(self, capability: Capability) -> bool {
        match capability.0 {
            1 => self.echo,
            2 => self.private_relay_provider,
            // Remote-service capabilities are governed by signed member grants alone.
            4 | 8 | 16 | 32 | 64 => true,
            _ => false,
        }
    }

    pub(crate) fn maximum_members(self) -> usize {
        usize::from(self.maximum_members)
    }

    pub(crate) fn encode(self, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        write_map(output, 3)?;
        write_uint(output, 0);
        write_bool(output, self.echo);
        write_uint(output, 1);
        write_bool(output, self.private_relay_provider);
        write_uint(output, 2);
        write_uint(output, u64::from(self.maximum_members));
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.map(3)?;
        decoder.key(0)?;
        let echo = decoder.boolean()?;
        decoder.key(1)?;
        let private_relay_provider = decoder.boolean()?;
        decoder.key(2)?;
        let maximum_members =
            u8::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        if maximum_members == 0 || usize::from(maximum_members) > MAX_SPACE_MEMBERS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            echo,
            private_relay_provider,
            maximum_members,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Version 1 membership record for one Runtime endpoint.
pub struct SpaceMemberV1 {
    endpoint_id: EndpointId,
    label: String,
    capabilities: MemberCapabilities,
}

impl SpaceMemberV1 {
    /// Creates a validated member record.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for an empty, oversized, or control-bearing label.
    pub fn new(
        endpoint_id: EndpointId,
        label: String,
        capabilities: MemberCapabilities,
    ) -> Result<Self, ProtocolError> {
        if label.is_empty()
            || label.len() > MAX_MEMBER_LABEL_LEN
            || label.chars().any(char::is_control)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            endpoint_id,
            label,
            capabilities,
        })
    }

    /// Returns the member endpoint identifier.
    pub const fn endpoint_id(&self) -> EndpointId {
        self.endpoint_id
    }
    /// Returns the human-readable member label.
    pub fn label(&self) -> &str {
        &self.label
    }
    /// Returns the member capability grants.
    pub const fn capabilities(&self) -> MemberCapabilities {
        self.capabilities
    }

    pub(crate) fn encode(&self, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        write_map(output, 3)?;
        write_uint(output, 0);
        write_bytes(output, self.endpoint_id.as_bytes())?;
        write_uint(output, 1);
        write_text(output, &self.label)?;
        write_uint(output, 2);
        write_uint(output, u64::from(self.capabilities.bits()));
        Ok(())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        decoder.map(3)?;
        decoder.key(0)?;
        let endpoint_id = EndpointId::try_from(decoder.bytes(32)?)?;
        decoder.key(1)?;
        let label = decoder.text(MAX_MEMBER_LABEL_LEN)?.to_owned();
        decoder.key(2)?;
        let bits = u16::try_from(decoder.uint()?).map_err(|_| ProtocolError::INVALID_INPUT)?;
        Self::new(endpoint_id, label, MemberCapabilities::from_bits(bits)?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Version 1 revocation of one endpoint from a Space.
pub struct SpaceRevocationV1 {
    endpoint_id: EndpointId,
}

impl SpaceRevocationV1 {
    /// Creates a revocation for `endpoint_id`.
    pub const fn new(endpoint_id: EndpointId) -> Self {
        Self { endpoint_id }
    }
    /// Returns the revoked endpoint identifier.
    pub const fn endpoint_id(self) -> EndpointId {
        self.endpoint_id
    }

    pub(crate) fn encode(self, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        write_bytes(output, self.endpoint_id.as_bytes())
    }

    pub(crate) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        Ok(Self::new(EndpointId::try_from(decoder.bytes(32)?)?))
    }
}

pub(crate) fn encode_members(
    output: &mut Vec<u8>,
    members: &[SpaceMemberV1],
) -> Result<(), ProtocolError> {
    write_array(output, members.len())?;
    for member in members {
        member.encode(output)?;
    }
    Ok(())
}
