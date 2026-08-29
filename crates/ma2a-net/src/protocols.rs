use ma2a_core::RemoteOperation;

/// Reserved bootstrap-only enrollment ALPN.
pub const ENROLLMENT_ALPN: &[u8] = b"ma2a/enrollment/1";
/// The complete ALPN set exposed by an Endpoint with no current Space memberships.
pub const ZERO_SPACE_ALPNS: [&[u8]; 1] = [ENROLLMENT_ALPN];
/// Normal MA2A ALPNs rejected by a zero-Space Runtime during TLS negotiation.
pub const NORMAL_PROTOCOL_ALPNS: [&[u8]; 5] = [
    b"ma2a/echo/1",
    b"ma2a/control/1",
    b"ma2a/metadata/1",
    b"ma2a/address/1",
    b"ma2a/relay/1",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoleKind {
    Enrollment,
    Normal(RemoteOperation),
}

/// Closed fail-closed routing classification applied before payload parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolRole(RoleKind);

impl ProtocolRole {
    /// The only role accepted while the Runtime has no Space memberships.
    pub const ENROLLMENT: Self = Self(RoleKind::Enrollment);
    /// A generic normal role retained for role-only callers; it never authorizes a service.
    pub const NORMAL: Self = Self(RoleKind::Normal(RemoteOperation::UNSUPPORTED));

    /// Classifies known ALPNs and rejects unknown or future values.
    pub fn from_alpn(alpn: &[u8]) -> Option<Self> {
        if alpn == ENROLLMENT_ALPN {
            Some(Self::ENROLLMENT)
        } else if alpn == NORMAL_PROTOCOL_ALPNS[0] {
            Some(Self(RoleKind::Normal(RemoteOperation::ECHO_CALL)))
        } else if alpn == NORMAL_PROTOCOL_ALPNS[1] {
            Some(Self(RoleKind::Normal(RemoteOperation::CONTROL_SYNC)))
        } else if alpn == NORMAL_PROTOCOL_ALPNS[2] {
            Some(Self(RoleKind::Normal(RemoteOperation::METADATA_READ)))
        } else if alpn == NORMAL_PROTOCOL_ALPNS[3] {
            Some(Self(RoleKind::Normal(
                RemoteOperation::ADDRESS_RECORD_EXCHANGE,
            )))
        } else if alpn == NORMAL_PROTOCOL_ALPNS[4] {
            Some(Self(RoleKind::Normal(RemoteOperation::RELAY_ADVERTISEMENT)))
        } else {
            None
        }
    }

    /// Returns whether this is the reserved enrollment role.
    pub const fn is_enrollment(self) -> bool {
        matches!(self.0, RoleKind::Enrollment)
    }

    /// Returns whether the role must pass the central remote authorization entry point.
    pub const fn requires_remote_authorization(self) -> bool {
        matches!(self.0, RoleKind::Normal(_))
    }

    /// Returns the closed operation required by a known normal protocol.
    pub const fn operation(self) -> Option<RemoteOperation> {
        match self.0 {
            RoleKind::Enrollment => None,
            RoleKind::Normal(operation) => Some(operation),
        }
    }
}
