//! A responder answering a full 64-Space request still sends full-sized address records.

use iroh::SecretKey;
use iroh_base::{CustomAddr, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactKind,
    ControlCursorV1, ControlRequestV1, ControlResponseV1, EndpointId, MAX_ADDRESS_RECORD_LEN,
    MAX_CONTROL_BATCH_BYTES, MAX_CONTROL_SPACES_PER_REQUEST, SpaceAddressRecordV1, SpaceId,
};
use ma2a_store::{Repository, StoreConfig};

use super::round_tests::{TempState, TestResult, member, share_space_with};

/// Endpoint data whose signed address record is as large as the protocol accepts.
fn full_record_data(peer: &SecretKey) -> TestResult<(AddressEndpointDataV1, usize)> {
    let scope = AddressRecordScope::new(
        SpaceId::try_from([0x44; 32].as_slice())?,
        peer.public().into(),
    );
    for last in (0..=1_024_usize).rev() {
        let addresses = (0..16_u64)
            .map(|id| {
                let len = if id == 15 { last } else { 1_024 };
                TransportAddr::Custom(CustomAddr::from_parts(id, &vec![0x5a; len]))
            })
            .collect();
        let data = AddressEndpointDataV1::new(addresses)?;
        // Signing refuses a record over the limit; the first one it accepts is the largest.
        let signed =
            SpaceAddressRecordV1::new(scope, AddressRecordValidity::new(1, 12, 100)?, data.clone())
                .sign(peer);
        if let Ok(signed) = signed {
            let len = signed.canonical_bytes().len();
            assert!(len <= MAX_ADDRESS_RECORD_LEN);
            return Ok((data, len));
        }
    }
    Err("no padding gives a full-sized record".into())
}

#[test]
fn a_full_request_still_carries_full_sized_address_records() -> TestResult {
    // Given: 64 shared Spaces, each holding one address record of the largest size the
    // protocol accepts, larger than an equal 64th of the response.
    let state = TempState::new("full-record")?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x11; 32]).public());
    let local = member(local_id, "local")?;
    let peer = SecretKey::from_bytes(&[0x22; 32]);
    let (data, record_len) = full_record_data(&peer)?;
    let share = super::pages::response_artifact_budget(MAX_CONTROL_SPACES_PER_REQUEST)
        .map_err(|rejection| format!("{rejection:?}"))?;
    assert!(
        record_len + 5 > share,
        "{record_len} fits a {share}-byte share"
    );
    for _space in 0..MAX_CONTROL_SPACES_PER_REQUEST {
        share_space_with(&mut repository, (&local, &peer), data.clone())?;
    }
    let cursors = repository
        .control_spaces_for(local_id)?
        .iter()
        .map(|shared| {
            let chain = shared.chain();
            ControlCursorV1::new(
                chain.space_id(),
                chain.latest_generation(),
                chain.latest_hash(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(cursors.len(), MAX_CONTROL_SPACES_PER_REQUEST);
    let input = super::ControlExchangeInput {
        local_endpoint_id: local_id,
        remote_endpoint_id: EndpointId::from(peer.public()),
        payload: ControlRequestV1::new(cursors)?.encode()?,
        now_ms: 50,
    };

    // When
    let outcome = super::respond(&mut repository, &input).map_err(|failure| failure.to_string())?;

    // Then: the response stays within one frame and carries as many of the records as fit it,
    // so the peer advances instead of being told there is more on every round.
    let response_bytes = outcome.response.map_err(|failure| failure.to_string())?;
    assert!(response_bytes.len() <= MAX_CONTROL_BATCH_BYTES);
    let response = ControlResponseV1::decode(&response_bytes)?;
    let delivered = response
        .pages()
        .iter()
        .flat_map(ma2a_core::ControlPageV1::artifacts)
        .filter(|artifact| artifact.kind() == ControlArtifactKind::ADDRESS_RECORD)
        .count();
    assert_eq!(delivered, MAX_CONTROL_SPACES_PER_REQUEST - 1);
    Ok(())
}

#[test]
fn respond_preserves_committed_changes_when_post_apply_cursor_validation_fails() -> TestResult {
    use ma2a_core::{
        ControlArtifactV1, ControlPageV1, MemberCapabilities, SpaceAuthoritySecret, SpaceChain,
        SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceManifestLink,
        SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
    };
    use ma2a_net::ControlRejection;

    let state = TempState::new("post-apply-cursor")?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let authority = SpaceAuthoritySecret::from_bytes([0x61; 32]);
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x62; 32]).public());
    let peer = SecretKey::from_bytes(&[0x63; 32]);
    let peer_id = EndpointId::from(peer.public());
    let caps = MemberCapabilities::new(true, true);
    let local_member = SpaceMemberV1::new(local_id, "local".to_owned(), caps)?;
    let peer_member = SpaceMemberV1::new(peer_id, "peer".to_owned(), caps)?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x64; 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(local_member.clone(), SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let mut members1 = vec![local_member.clone(), peer_member];
    members1.sort_by_key(SpaceMemberV1::endpoint_id);
    let mut chain = SpaceChain::from_genesis(genesis)?;
    let m1 = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        10,
        SpaceManifestMembership::new(members1, Vec::new()),
    )?
    .sign(&authority)?;
    chain.apply(&m1)?;
    repository.persist_space_chain(&chain)?;

    let m2 = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        20,
        SpaceManifestMembership::new(vec![local_member], vec![SpaceRevocationV1::new(peer_id)]),
    )?
    .sign(&authority)?;
    chain.apply(&m2)?;
    let page = ControlPageV1::new(
        chain.space_id(),
        false,
        vec![ControlArtifactV1::new(
            ControlArtifactKind::MANIFEST,
            m2.canonical_bytes().to_vec(),
        )?],
    )?;
    let cursor = ControlCursorV1::new(chain.space_id(), 2, chain.latest_hash());
    let payload = ControlRequestV1::new(vec![cursor])?
        .with_push_pages(vec![page])?
        .encode()?;
    let before_rev = repository.revision()?;

    let outcome = super::respond(
        &mut repository,
        &super::ControlExchangeInput {
            local_endpoint_id: local_id,
            remote_endpoint_id: peer_id,
            payload,
            now_ms: 50,
        },
    )
    .map_err(|failure| failure.to_string())?;

    assert!(matches!(
        outcome.response,
        Err(super::ControlFailure::Rejected(ControlRejection::Invalid))
    ));
    assert!(outcome.revision > before_rev);
    assert_eq!(outcome.revision, repository.revision()?);
    assert_eq!(
        outcome.changes,
        super::ControlChanges {
            manifest: true,
            address: false,
            relay: false,
        }
    );
    Ok(())
}

#[test]
fn respond_preserves_committed_address_when_pull_cursor_mismatches() -> TestResult {
    use ma2a_core::{
        AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, ControlArtifactV1,
        ControlPageV1, SpaceAddressRecordV1,
    };
    use ma2a_net::ControlRejection;

    let state = TempState::new("post-apply-address")?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let local_id = EndpointId::from(SecretKey::from_bytes(&[0x71; 32]).public());
    let peer = SecretKey::from_bytes(&[0x72; 32]);
    let peer_id = EndpointId::from(peer.public());
    share_space_with(
        &mut repository,
        (&member(local_id, "local")?, &peer),
        AddressEndpointDataV1::new(Vec::new())?,
    )?;
    let space_id = repository
        .control_spaces_for(local_id)?
        .first()
        .ok_or("missing shared space")?
        .chain()
        .space_id();
    let signed_addr = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, peer_id),
        AddressRecordValidity::new(2, 20, 200)?,
        AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(&peer)?;
    let page = ControlPageV1::new(
        space_id,
        false,
        vec![ControlArtifactV1::new(
            ControlArtifactKind::ADDRESS_RECORD,
            signed_addr.canonical_bytes().to_vec(),
        )?],
    )?;
    let bad_cursor = ControlCursorV1::new(space_id, 1, [0x5a; 32]);
    let payload = ControlRequestV1::new(vec![bad_cursor])?
        .with_push_pages(vec![page])?
        .encode()?;
    let before_rev = repository.revision()?;

    let outcome = super::respond(
        &mut repository,
        &super::ControlExchangeInput {
            local_endpoint_id: local_id,
            remote_endpoint_id: peer_id,
            payload,
            now_ms: 50,
        },
    )
    .map_err(|failure| failure.to_string())?;

    assert!(matches!(
        outcome.response,
        Err(super::ControlFailure::Rejected(ControlRejection::Invalid))
    ));
    assert!(outcome.revision > before_rev);
    assert_eq!(outcome.revision, repository.revision()?);
    assert_eq!(
        outcome.changes,
        super::ControlChanges {
            manifest: false,
            address: true,
            relay: false,
        }
    );
    Ok(())
}
