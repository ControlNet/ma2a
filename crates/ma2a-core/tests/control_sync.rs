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
