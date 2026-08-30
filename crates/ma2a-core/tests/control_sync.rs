//! Canonical bounded control synchronization wire tests.

use iroh_base::SecretKey;
use ma2a_core::{
    ControlArtifactKind, ControlArtifactV1, ControlCursorEntryV1, ControlCursorV1, ControlPageV1,
    ControlRequestV1, ControlResponseV1, EndpointId, MAX_CONTROL_ARTIFACTS_PER_PAGE,
    MAX_CONTROL_BATCH_BYTES, SpaceId,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn bounded_control_messages_round_trip_canonically() -> TestResult {
    // Given
    let endpoint_id: EndpointId = SecretKey::from_bytes(&[0x31; 32]).public().into();
    let space_id = SpaceId::try_from([0x42; 32].as_slice())?;
    let cursor = ControlCursorV1::new(space_id, 9, [0x51; 32]).with_entries(
        vec![ControlCursorEntryV1::new(endpoint_id, 3)],
        vec![ControlCursorEntryV1::new(endpoint_id, 7)],
    )?;
    let page = ControlPageV1::new(
        space_id,
        false,
        vec![ControlArtifactV1::new(
            ControlArtifactKind::MANIFEST,
            vec![0x61; 128],
        )?],
    )?;
    let request = ControlRequestV1::new(vec![cursor])?.with_push_pages(vec![page.clone()])?;
    let response = ControlResponseV1::new(vec![page])?;

    // When
    let request_bytes = request.encode()?;
    let response_bytes = response.encode()?;

    // Then
    assert_eq!(ControlRequestV1::decode(&request_bytes)?, request);
    assert_eq!(ControlResponseV1::decode(&response_bytes)?, response);
    assert_eq!(
        ControlRequestV1::decode(&request_bytes)?.encode()?,
        request_bytes
    );
    assert_eq!(
        ControlResponseV1::decode(&response_bytes)?.encode()?,
        response_bytes
    );
    Ok(())
}

#[test]
fn control_messages_reject_trailing_and_oversized_batches() -> TestResult {
    // Given
    let request = ControlRequestV1::new(Vec::new())?;
    let mut trailing = request.encode()?;
    trailing.push(0);
    let space_id = SpaceId::try_from([0x52; 32].as_slice())?;
    let too_many = (0..=MAX_CONTROL_ARTIFACTS_PER_PAGE)
        .map(|_| ControlArtifactV1::new(ControlArtifactKind::ADDRESS_RECORD, vec![1]))
        .collect::<Result<Vec<_>, _>>()?;
    let oversized = ControlArtifactV1::new(
        ControlArtifactKind::RELAY_ADVERTISEMENT,
        vec![0; MAX_CONTROL_BATCH_BYTES + 1],
    );

    // When
    let trailing_result = ControlRequestV1::decode(&trailing);
    let page_result = ControlPageV1::new(space_id, false, too_many);

    // Then
    assert!(trailing_result.is_err());
    assert!(page_result.is_err());
    assert!(oversized.is_err());
    Ok(())
}

#[test]
fn response_pages_reject_unsorted_and_duplicate_spaces() -> TestResult {
    // Given
    let space_a = SpaceId::try_from([0x41; 32].as_slice())?;
    let space_b = SpaceId::try_from([0x42; 32].as_slice())?;
    let page_a = ControlPageV1::new(space_a, false, Vec::new())?;
    let page_b = ControlPageV1::new(space_b, false, Vec::new())?;

    // When
    let unsorted = ControlResponseV1::new(vec![page_b, page_a.clone()]);
    let duplicate = ControlResponseV1::new(vec![page_a.clone(), page_a]);

    // Then
    assert!(unsorted.is_err());
    assert!(duplicate.is_err());
    Ok(())
}

#[test]
fn decoders_reject_noncanonical_or_unbound_page_spaces() {
    // Given
    let space_a = [0x41; 32];
    let space_b = [0x42; 32];
    let mut unsorted_response = b"MCR1".to_vec();
    unsorted_response.extend_from_slice(&2_u16.to_be_bytes());
    append_empty_page(&mut unsorted_response, space_b);
    append_empty_page(&mut unsorted_response, space_a);
    let mut duplicate_response = b"MCR1".to_vec();
    duplicate_response.extend_from_slice(&2_u16.to_be_bytes());
    append_empty_page(&mut duplicate_response, space_a);
    append_empty_page(&mut duplicate_response, space_a);
    let mut unbound_request = b"MCQ1".to_vec();
    unbound_request.extend_from_slice(&1_u16.to_be_bytes());
    unbound_request.extend_from_slice(&space_a);
    unbound_request.extend_from_slice(&0_u64.to_be_bytes());
    unbound_request.extend_from_slice(&[0; 32]);
    unbound_request.extend_from_slice(&0_u16.to_be_bytes());
    unbound_request.extend_from_slice(&0_u16.to_be_bytes());
    unbound_request.extend_from_slice(&1_u16.to_be_bytes());
    append_empty_page(&mut unbound_request, space_b);

    // When
    let unsorted_result = ControlResponseV1::decode(&unsorted_response);
    let duplicate_result = ControlResponseV1::decode(&duplicate_response);
    let unbound_result = ControlRequestV1::decode(&unbound_request);

    // Then
    assert!(unsorted_result.is_err());
    assert!(duplicate_result.is_err());
    assert!(unbound_result.is_err());
}

fn append_empty_page(bytes: &mut Vec<u8>, space_id: [u8; 32]) {
    bytes.extend_from_slice(&space_id);
    bytes.push(0);
    bytes.extend_from_slice(&0_u16.to_be_bytes());
}
