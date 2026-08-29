use iroh_tickets::{Ticket as _, endpoint::EndpointTicket};

use super::{
    EndpointId, FIXED_INVITE_BODY_LEN, INVITE_VERSION, InviteValidity, MAX_INVITE_TICKET_BYTES,
    ProtocolError, SignedInviteTicket, SpaceId,
};

pub(super) fn decode_ticket(bytes: &[u8]) -> Result<SignedInviteTicket, ProtocolError> {
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
    let _validity = InviteValidity::new(created_at_ms, expires_at_ms)?;
    let owner_addr_len = u16::from_be_bytes(take::<2>(bytes, &mut cursor)?);
    let endpoint_end = cursor
        .checked_add(usize::from(owner_addr_len))
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let owner_addr_bytes = bytes
        .get(cursor..endpoint_end)
        .ok_or(ProtocolError::INVALID_INPUT)?
        .to_vec();
    cursor = endpoint_end;
    let owner_addr = EndpointTicket::decode_bytes(&owner_addr_bytes)
        .map_err(|_| ProtocolError::INVALID_INPUT)?
        .endpoint_addr()
        .clone();
    let secret = take_slice(bytes, &mut cursor, 32)?;
    let signature = take_slice(bytes, &mut cursor, 64)?;
    if cursor != bytes.len() {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let secret = <[u8; 32]>::try_from(secret).map_err(|_| ProtocolError::INVALID_INPUT)?;
    let signature = <[u8; 64]>::try_from(signature).map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok(SignedInviteTicket {
        invitation_id,
        space_id,
        creator,
        created_at_ms,
        expires_at_ms,
        owner_addr,
        owner_addr_len,
        owner_addr_bytes,
        secret,
        signature,
    })
}

fn take<const N: usize>(bytes: &[u8], cursor: &mut usize) -> Result<[u8; N], ProtocolError> {
    <[u8; N]>::try_from(take_slice(bytes, cursor, N)?).map_err(|_| ProtocolError::INVALID_INPUT)
}

fn take_slice<'a>(
    bytes: &'a [u8],
    cursor: &mut usize,
    length: usize,
) -> Result<&'a [u8], ProtocolError> {
    let end = cursor
        .checked_add(length)
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let value = bytes
        .get(*cursor..end)
        .ok_or(ProtocolError::INVALID_INPUT)?;
    *cursor = end;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use iroh_base::{EndpointAddr, SecretKey};
    use iroh_tickets::Ticket as _;

    use super::decode_ticket;
    use crate::{
        EndpointId, InviteEntropy, InviteValidity, SignedInviteTicket, SpaceAuthoritySecret,
        SpaceId,
    };

    #[test]
    fn trailing_bytes_after_signature_are_rejected() {
        // Given
        let endpoint_secret = SecretKey::generate();
        let endpoint_id = EndpointId::from(endpoint_secret.public());
        let authority = SpaceAuthoritySecret::from_bytes([0x91; 32]);
        let ticket = SignedInviteTicket::sign(
            SpaceId::derive(b"codec-test-space"),
            endpoint_id,
            EndpointAddr::new(endpoint_secret.public()),
            InviteValidity::new(100, 200).expect("valid test interval"),
            &InviteEntropy::from_bytes([0x92; 16], [0x93; 32]),
            &authority,
        )
        .expect("valid test ticket");
        let mut malformed = ticket.encode_bytes();
        malformed.push(0);

        // When
        let result = decode_ticket(&malformed);

        // Then
        assert!(result.is_err());
    }
}
