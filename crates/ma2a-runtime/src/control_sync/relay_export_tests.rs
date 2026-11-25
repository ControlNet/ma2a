//! A relay advertisement or address record stored before the relay URL policy was tightened is
//! not sent to peers, which would refuse the whole page that carries it.

use iroh::SecretKey;
use iroh_base::TransportAddr;
use ma2a_core::{
    AddressEndpointDataV1, ControlArtifactKind, ControlCursorV1, ControlRequestV1,
    ControlResponseV1, EndpointId, PRIVATE_RELAY_ADVERTISEMENT_SIGNATURE_DOMAIN,
    PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity,
};
use ma2a_store::{Repository, StoreConfig, ValidatedRelayAdvertisement};

use super::round_tests::{TempState, TestResult, member, share_space_with};

/// Two URLs of one length: one the policy accepts, and one with credentials it now refuses.
const ACCEPTED: &str = "https://relay-padding-xxx.invalid/";
const REFUSED: &str = "https://user:secret@relay.invalid/";

/// `signed` with its relay URL replaced by [`REFUSED`] and signed again by `provider`, as an
/// earlier version accepted it.
fn with_refused_url(signed: &[u8], provider: &SecretKey) -> TestResult<Vec<u8>> {
    let at = signed
        .windows(ACCEPTED.len())
        .position(|window| window == ACCEPTED.as_bytes())
        .ok_or("relay URL in the advertisement")?;
    let mut patched = signed.to_vec();
    patched
        .get_mut(at..at + REFUSED.len())
        .ok_or("URL range")?
        .copy_from_slice(REFUSED.as_bytes());
    // The bytes are `{0: body, 1: signature}`: the body follows its byte-string header and
    // ends before the last key, its header and the 64 signature bytes.
    let header = match signed.get(2) {
        Some(0x58) => 2,
        Some(0x59) => 3,
        _ => return Err("unexpected body header".into()),
    };
    let signature_at = patched.len() - 64;
    let mut message = PRIVATE_RELAY_ADVERTISEMENT_SIGNATURE_DOMAIN.to_vec();
    message.extend_from_slice(patched.get(2 + header..signature_at - 3).ok_or("body")?);
    patched
        .get_mut(signature_at..)
        .ok_or("signature range")?
        .copy_from_slice(&provider.sign(&message).to_bytes());
    Ok(patched)
}

#[test]
fn a_stored_advertisement_the_policy_now_refuses_is_not_exported() -> TestResult {
    // Given: a Space shared with a relay provider, holding the provider's address record and
    // its relay advertisement, whose URL carries credentials as an earlier version allowed.
    assert_eq!(ACCEPTED.len(), REFUSED.len());
    let state = TempState::new("refused-advertisement")?;
    let config = StoreConfig::new(&state.0);
    let mut repository = Repository::open(&config)?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x11; 32]).public());
    let provider = SecretKey::from_bytes(&[0x22; 32]);
    let provider_id = EndpointId::from(provider.public());
    share_space_with(
        &mut repository,
        (&member(local_id, "local")?, &provider),
        AddressEndpointDataV1::new(Vec::new())?,
    )?;
    let shared = repository.control_spaces_for(local_id)?;
    let shared = shared.first().ok_or("shared Space")?;
    let space_id = shared.chain().space_id();
    let signed = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(space_id, provider_id),
        ACCEPTED.parse()?,
        PrivateRelayAdvertisementValidity::new(1, 12, 100)?,
    )?
    .sign(&provider)?;
    let validated =
        ValidatedRelayAdvertisement::parse(signed.canonical_bytes(), &shared.authorization(), 50)
            .map_err(|error| format!("{error:?}"))?;
    repository.advance_private_relay_advertisement(&validated)?;
    let cursor = ControlCursorV1::new(
        space_id,
        shared.chain().latest_generation(),
        shared.chain().latest_hash(),
    );
    drop(repository);
    rusqlite::Connection::open(config.database_path())?.execute(
        "UPDATE relay_advertisement_state SET signed_advertisement = ?1",
        [with_refused_url(signed.canonical_bytes(), &provider)?],
    )?;
    let mut repository = Repository::open(&config)?;

    // When: the provider asks for what it is missing.
    let outcome = super::respond(
        &mut repository,
        &super::ControlExchangeInput {
            local_endpoint_id: local_id,
            remote_endpoint_id: provider_id,
            payload: ControlRequestV1::new(vec![cursor])?.encode()?,
            now_ms: 50,
        },
    )
    .map_err(|failure| failure.to_string())?;

    // Then: the address record is sent, and the refused advertisement is not.
    let response = ControlResponseV1::decode(&outcome.response)?;
    let kinds = response
        .pages()
        .iter()
        .flat_map(ma2a_core::ControlPageV1::artifacts)
        .map(ma2a_core::ControlArtifactV1::kind)
        .collect::<Vec<_>>();
    assert_eq!(kinds, vec![ControlArtifactKind::ADDRESS_RECORD]);
    Ok(())
}

#[test]
fn a_stored_address_record_the_policy_now_refuses_is_skipped_and_not_exported() -> TestResult {
    // Given: a Space shared with a peer whose stored address record carries a relay URL that
    // names a user, as an earlier version allowed.
    let state = TempState::new("refused-address-record")?;
    let config = StoreConfig::new(&state.0);
    let mut repository = Repository::open(&config)?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x11; 32]).public());
    let peer = SecretKey::from_bytes(&[0x22; 32]);
    share_space_with(
        &mut repository,
        (&member(local_id, "local")?, &peer),
        AddressEndpointDataV1::new(vec![TransportAddr::Relay(ACCEPTED.parse()?)])?,
    )?;
    let shared = repository.control_spaces_for(local_id)?;
    let shared = shared.first().ok_or("shared Space")?;
    let cursor = ControlCursorV1::new(
        shared.chain().space_id(),
        shared.chain().latest_generation(),
        shared.chain().latest_hash(),
    );
    let stored = shared
        .address_records()
        .first()
        .ok_or("stored record")?
        .signed_record()
        .to_vec();
    let at = stored
        .windows(ACCEPTED.len())
        .position(|window| window == ACCEPTED.as_bytes())
        .ok_or("relay URL in the record")?;
    let mut refused = stored;
    refused
        .get_mut(at..at + REFUSED.len())
        .ok_or("URL range")?
        .copy_from_slice(REFUSED.as_bytes());
    drop(repository);
    rusqlite::Connection::open(config.database_path())?
        .execute("UPDATE address_state SET signed_record = ?1", [refused])?;
    let mut repository = Repository::open(&config)?;

    // When: the Runtime loads its lookup, as at startup, and answers the peer.
    let lookup = super::load_lookup(&mut repository, local_id, 50);
    let outcome = super::respond(
        &mut repository,
        &super::ControlExchangeInput {
            local_endpoint_id: local_id,
            remote_endpoint_id: EndpointId::from(peer.public()),
            payload: ControlRequestV1::new(vec![cursor])?.encode()?,
            now_ms: 50,
        },
    )
    .map_err(|failure| failure.to_string())?;

    // Then: the record is left out of both, and neither fails.
    assert!(lookup.is_ok_and(|lookup| lookup.addresses.is_empty()));
    let response = ControlResponseV1::decode(&outcome.response)?;
    assert!(
        response
            .pages()
            .iter()
            .all(|page| page.artifacts().is_empty())
    );
    Ok(())
}
