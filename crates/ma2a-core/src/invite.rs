use std::fmt;

use ed25519_dalek::Signature;
use iroh_base::EndpointAddr;
use iroh_tickets::{Ticket, endpoint::EndpointTicket};
use zeroize::Zeroize as _;

use crate::{EndpointId, ProtocolError, SpaceAuthorityPublicKey, SpaceAuthoritySecret, SpaceId};

const INVITE_SIGNATURE_DOMAIN: &[u8] = b"ma2a-enrollment-invite-signature-v1";
const INVITE_DIGEST_DOMAIN: &[u8] = b"ma2a-enrollment-invite-digest-v1";
const INVITE_VERSION: u8 = 1;
const FIXED_INVITE_BODY_LEN: usize = 1 + 16 + 32 + 32 + 8 + 8 + 2 + 32;
/// Maximum encoded invitation ticket bytes before base32 wrapping.
pub const MAX_INVITE_TICKET_BYTES: usize = 1_024;

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
        if validity.expires_at_ms <= validity.created_at_ms
            || owner_addr.id != creator.to_public_key()?
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut ticket = Self {
            invitation_id: entropy.invitation_id,
            space_id,
            creator,
            created_at_ms: validity.created_at_ms,
            expires_at_ms: validity.expires_at_ms,
            owner_addr,
            secret: entropy.secret,
            signature: [0; 64],
        };
        ticket.signature = authority.sign(INVITE_SIGNATURE_DOMAIN, &ticket.body_bytes()?);
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
            || self.expires_at_ms <= self.created_at_ms
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut message = Vec::with_capacity(INVITE_SIGNATURE_DOMAIN.len() + FIXED_INVITE_BODY_LEN);
        message.extend_from_slice(INVITE_SIGNATURE_DOMAIN);
        message.extend_from_slice(&self.body_bytes()?);
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

    fn body_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        let endpoint = EndpointTicket::new(self.owner_addr.clone()).encode_bytes();
        let endpoint_len =
            u16::try_from(endpoint.len()).map_err(|_| ProtocolError::INVALID_INPUT)?;
        let mut output = Vec::with_capacity(FIXED_INVITE_BODY_LEN + endpoint.len());
        output.push(INVITE_VERSION);
        output.extend_from_slice(&self.invitation_id);
        output.extend_from_slice(self.space_id.as_bytes());
        output.extend_from_slice(self.creator.as_bytes());
        output.extend_from_slice(&self.created_at_ms.to_be_bytes());
        output.extend_from_slice(&self.expires_at_ms.to_be_bytes());
        output.extend_from_slice(&endpoint_len.to_be_bytes());
        output.extend_from_slice(&endpoint);
        output.extend_from_slice(&self.secret);
        Ok(output)
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

impl Ticket for SignedInviteTicket {
    const KIND: &'static str = "ma2ainvite";

    fn encode_bytes(&self) -> Vec<u8> {
        let mut output = self.body_bytes().unwrap_or_default();
        output.extend_from_slice(&self.signature);
        output
    }

    fn decode_bytes(bytes: &[u8]) -> Result<Self, iroh_tickets::ParseError> {
        decode_ticket(bytes)
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
    pub const fn new(created_at_ms: u64, expires_at_ms: u64) -> Self {
        Self {
            created_at_ms,
            expires_at_ms,
        }
    }
}

fn decode_ticket(bytes: &[u8]) -> Result<SignedInviteTicket, ProtocolError> {
    if bytes.len() > MAX_INVITE_TICKET_BYTES || bytes.len() < FIXED_INVITE_BODY_LEN + 64 {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let mut cursor = 0;
    if take::<1>(bytes, &mut cursor)?[0] != INVITE_VERSION {
        return Err(ProtocolError::VERSION_MISMATCH);
    }
    let invitation_id = take::<16>(bytes, &mut cursor)?;
    let space_id = SpaceId::try_from(take::<32>(bytes, &mut cursor)?.as_slice())?;
    let creator = EndpointId::try_from(take::<32>(bytes, &mut cursor)?.as_slice())?;
    let created_at_ms = u64::from_be_bytes(take::<8>(bytes, &mut cursor)?);
    let expires_at_ms = u64::from_be_bytes(take::<8>(bytes, &mut cursor)?);
    let endpoint_len = usize::from(u16::from_be_bytes(take::<2>(bytes, &mut cursor)?));
    let endpoint_end = cursor
        .checked_add(endpoint_len)
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let endpoint = bytes
        .get(cursor..endpoint_end)
        .ok_or(ProtocolError::INVALID_INPUT)?;
    cursor = endpoint_end;
    let owner_addr = EndpointTicket::decode_bytes(endpoint)
        .map_err(|_| ProtocolError::INVALID_INPUT)?
        .endpoint_addr()
        .clone();
    let secret = take::<32>(bytes, &mut cursor)?;
    let signature = take::<64>(bytes, &mut cursor)?;
    if cursor != bytes.len() {
        return Err(ProtocolError::INVALID_INPUT);
    }
    Ok(SignedInviteTicket {
        invitation_id,
        space_id,
        creator,
        created_at_ms,
        expires_at_ms,
        owner_addr,
        secret,
        signature,
    })
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
