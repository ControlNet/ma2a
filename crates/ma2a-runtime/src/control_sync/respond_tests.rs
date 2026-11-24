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
    assert!(outcome.response.len() <= MAX_CONTROL_BATCH_BYTES);
    let response = ControlResponseV1::decode(&outcome.response)?;
    let delivered = response
        .pages()
        .iter()
        .flat_map(ma2a_core::ControlPageV1::artifacts)
        .filter(|artifact| artifact.kind() == ControlArtifactKind::ADDRESS_RECORD)
        .count();
    assert_eq!(delivered, MAX_CONTROL_SPACES_PER_REQUEST - 1);
    Ok(())
}
