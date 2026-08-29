use std::fmt;

use ed25519_dalek::Signature;
use iroh_base::EndpointAddr;
use iroh_tickets::{Ticket, endpoint::EndpointTicket};
use zeroize::{Zeroize as _, Zeroizing};

use crate::{EndpointId, ProtocolError, SpaceAuthorityPublicKey, SpaceAuthoritySecret, SpaceId};

#[path = "invite/codec.rs"]
mod codec;

const INVITE_SIGNATURE_DOMAIN: &[u8] = b"ma2a-enrollment-invite-signature-v1";
const INVITE_DIGEST_DOMAIN: &[u8] = b"ma2a-enrollment-invite-digest-v1";
const INVITE_VERSION: u8 = 1;
const FIXED_INVITE_BODY_LEN: usize = 1 + 16 + 32 + 32 + 8 + 8 + 2 + 32;
/// Maximum encoded invitation ticket bytes before base32 wrapping.
pub const MAX_INVITE_TICKET_BYTES: usize = 1_024;
/// Maximum invitation validity interval in milliseconds.
pub const MAX_INVITE_LIFETIME_MS: u64 = 5 * 60 * 1_000;

#[derive(Clone, PartialEq, Eq)]
/// Caller-supplied entropy for deterministic invitation creation boundaries.
pub struct InviteEntropy {
    invitation_id: [u8; 16],
    secret: [u8; 32],
}

impl InviteEntropy {
    /// Creates explicit invitation entropy.
    pub const fn from_bytes(invitation_id: [u8; 16], secret: [u8; 32]) -> Self {
        Self {
            invitation_id,
            secret,
        }
    }

    /// Generates the required 48 random bytes from the operating system.
    ///
    /// # Errors
    ///
    /// Returns an error when the operating system cannot provide secure randomness.
    pub fn random() -> Result<Self, ProtocolError> {
        let mut bytes = [0_u8; 48];
        getrandom::fill(&mut bytes).map_err(|_| ProtocolError::INTERNAL)?;
        let invitation_id =
            <[u8; 16]>::try_from(&bytes[..16]).map_err(|_| ProtocolError::INTERNAL)?;
        let secret = <[u8; 32]>::try_from(&bytes[16..]).map_err(|_| ProtocolError::INTERNAL)?;
        bytes.zeroize();
        Ok(Self::from_bytes(invitation_id, secret))
    }
}

impl fmt::Debug for InviteEntropy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InviteEntropy([REDACTED])")
    }
}

impl Drop for InviteEntropy {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

#[derive(Clone, PartialEq, Eq)]
/// Compact authority-signed invitation carrying one single-use secret.
pub struct SignedInviteTicket {
    invitation_id: [u8; 16],
    space_id: SpaceId,
    creator: EndpointId,
    created_at_ms: u64,
    expires_at_ms: u64,
    owner_addr: EndpointAddr,
    owner_addr_len: u16,
    owner_addr_bytes: Vec<u8>,
    secret: [u8; 32],
    signature: [u8; 64],
}

impl SignedInviteTicket {
    /// Signs one bounded invitation with the repository-owned Space authority.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid validity, creator identity, address, or encoded size.
    #[allow(
        clippy::too_many_arguments,
        reason = "each signed field remains explicit at the authority boundary"
    )]
    pub fn sign(
        space_id: SpaceId,
        creator: EndpointId,
        owner_addr: EndpointAddr,
        validity: InviteValidity,
        entropy: &InviteEntropy,
        authority: &SpaceAuthoritySecret,
    ) -> Result<Self, ProtocolError> {
        if owner_addr.id != creator.to_public_key()? {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let owner_addr_bytes = EndpointTicket::new(owner_addr.clone()).encode_bytes();
        let owner_addr_len =
            u16::try_from(owner_addr_bytes.len()).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let mut ticket = Self {
            invitation_id: entropy.invitation_id,
            space_id,
            creator,
            created_at_ms: validity.created_at_ms,
            expires_at_ms: validity.expires_at_ms,
            owner_addr,
            owner_addr_len,
            owner_addr_bytes,
            secret: entropy.secret,
            signature: [0; 64],
        };
        ticket.signature = authority.sign(INVITE_SIGNATURE_DOMAIN, &ticket.body_bytes());
        if ticket.encode_bytes().len() > MAX_INVITE_TICKET_BYTES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(ticket)
    }

    /// Verifies the exact Space, authority signature, creator address, and validity interval.
    ///
    /// # Errors
    ///
    /// Returns an error when any signed field or the authority signature is invalid.
    pub fn verify(&self, authority: SpaceAuthorityPublicKey) -> Result<(), ProtocolError> {
        if self.owner_addr.id != self.creator.to_public_key()?
            || InviteValidity::new(self.created_at_ms, self.expires_at_ms).is_err()
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut message = Vec::with_capacity(INVITE_SIGNATURE_DOMAIN.len() + FIXED_INVITE_BODY_LEN);
        message.extend_from_slice(INVITE_SIGNATURE_DOMAIN);
        message.extend_from_slice(&self.body_bytes());
        ed25519_dalek::VerifyingKey::from_bytes(authority.as_bytes())
            .map_err(|_| ProtocolError::INVALID_INPUT)?
            .verify_strict(&message, &Signature::from_bytes(&self.signature))
            .map_err(|_| ProtocolError::INVALID_INPUT)
    }

    /// Returns the persisted domain-separated digest without exposing the secret.
    pub fn secret_digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(INVITE_DIGEST_DOMAIN);
        hasher.update(&self.secret);
        *hasher.finalize().as_bytes()
    }

    /// Returns the stable invitation identifier.
    pub const fn invitation_id(&self) -> [u8; 16] {
        self.invitation_id
    }
    /// Returns the invited Space identifier.
    pub const fn space_id(&self) -> SpaceId {
        self.space_id
    }
    /// Returns the owner endpoint that issued the invitation.
    pub const fn creator(&self) -> EndpointId {
        self.creator
    }
    /// Returns the issuance timestamp in milliseconds.
    pub const fn created_at_ms(&self) -> u64 {
        self.created_at_ms
    }
    /// Returns the expiry timestamp in milliseconds.
    pub const fn expires_at_ms(&self) -> u64 {
        self.expires_at_ms
    }
    /// Returns the routable owner endpoint address.
    pub fn owner_addr(&self) -> EndpointAddr {
        self.owner_addr.clone()
    }

    /// Returns the canonical bounded owner bootstrap address stored with the invitation.
    pub fn encoded_owner_addr(&self) -> &[u8] {
        &self.owner_addr_bytes
    }

    fn body_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut output = Zeroizing::new(Vec::with_capacity(
            FIXED_INVITE_BODY_LEN + self.owner_addr_bytes.len(),
        ));
        output.push(INVITE_VERSION);
        output.extend_from_slice(&self.invitation_id);
        output.extend_from_slice(self.space_id.as_bytes());
        output.extend_from_slice(self.creator.as_bytes());
        output.extend_from_slice(&self.created_at_ms.to_be_bytes());
        output.extend_from_slice(&self.expires_at_ms.to_be_bytes());
        output.extend_from_slice(&self.owner_addr_len.to_be_bytes());
        output.extend_from_slice(&self.owner_addr_bytes);
        output.extend_from_slice(&self.secret);
        output
    }
}

impl fmt::Debug for SignedInviteTicket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignedInviteTicket")
            .field("invitation_id", &self.invitation_id)
            .field("space_id", &self.space_id)
            .field("creator", &self.creator)
            .field("created_at_ms", &self.created_at_ms)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("secret", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl Drop for SignedInviteTicket {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl Ticket for SignedInviteTicket {
    const KIND: &'static str = "ma2ainvite";

    fn encode_bytes(&self) -> Vec<u8> {
        let mut output = self.body_bytes();
        output.extend_from_slice(&self.signature);
        output.to_vec()
    }

    fn decode_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        codec::decode_ticket(bytes)
            .map_err(|_| iroh_tickets::ParseError::verification_failed("invalid ma2a invite"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Bounded invitation creation and expiry timestamps.
pub struct InviteValidity {
    created_at_ms: u64,
    expires_at_ms: u64,
}

impl InviteValidity {
    /// Creates an issuance and expiry interval.
    ///
    /// # Errors
    /// Returns an error unless the interval is positive and at most five minutes.
    pub const fn new(created_at_ms: u64, expires_at_ms: u64) -> Result<Self, ProtocolError> {
        let Some(lifetime_ms) = expires_at_ms.checked_sub(created_at_ms) else {
            return Err(ProtocolError::INVALID_INPUT);
        };
        if lifetime_ms == 0 || lifetime_ms > MAX_INVITE_LIFETIME_MS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        Ok(Self {
            created_at_ms,
            expires_at_ms,
        })
    }

    /// Creates a validity interval from an owner-issued timestamp and bounded lifetime.
    ///
    /// # Errors
    /// Returns an error when the lifetime is zero, oversized, or overflows the owner timestamp.
    pub const fn for_lifetime(created_at_ms: u64, lifetime_ms: u64) -> Result<Self, ProtocolError> {
        if lifetime_ms == 0 || lifetime_ms > MAX_INVITE_LIFETIME_MS {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let Some(expires_at_ms) = created_at_ms.checked_add(lifetime_ms) else {
            return Err(ProtocolError::INVALID_INPUT);
        };
        Self::new(created_at_ms, expires_at_ms)
    }
}
