/// Reserved bootstrap-only enrollment ALPN.
pub const ENROLLMENT_ALPN: &[u8] = b"ma2a/enrollment/1";
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
    Normal,
}

/// Closed classification for the protocol roles known before service dispatch exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolRole(RoleKind);

impl ProtocolRole {
    /// The only role accepted while the Runtime has no Space memberships.
    pub const ENROLLMENT: Self = Self(RoleKind::Enrollment);
    /// A normal authenticated MA2A role requiring a shared Space.
    pub const NORMAL: Self = Self(RoleKind::Normal);

    /// Classifies a known ALPN without accepting unknown future protocols.
    pub fn from_alpn(alpn: &[u8]) -> Option<Self> {
        if alpn == ENROLLMENT_ALPN {
            Some(Self::ENROLLMENT)
        } else if NORMAL_PROTOCOL_ALPNS.contains(&alpn) {
            Some(Self::NORMAL)
        } else {
            None
        }
    }

    /// Returns whether this is the reserved enrollment role.
    pub const fn is_enrollment(self) -> bool {
        matches!(self.0, RoleKind::Enrollment)
    }
}
