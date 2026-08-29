//! Signed address validation and publisher policy coverage.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1,
};
use ma2a_net::{AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[test]
fn validator_accepts_only_the_requested_current_space_member() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x51; 32]);
    let fixture = space_fixture(&signer, 0x61)?;
    let state = TempState::new("validator-current-member")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let scope = AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into());
    let validity = AddressRecordValidity::new(1, NOW_MS, NOW_MS + 600_000)?;
    let envelope = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4101".parse()?)])?,
    )
    .sign(&signer)?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());

    // When
    let validated = AddressRecordValidator::validate_and_store(
        &mut repository,
        envelope.canonical_bytes(),
        target.validation(&fixture.authorization, NOW_MS),
    )?;

    // Then
    assert_eq!(validated.record(), &envelope);
    assert!(
        repository
            .address_record(fixture.genesis.space_id(), signer.public().into())?
            .is_some()
    );
    Ok(())
}

#[test]
fn validator_checks_target_before_signature_and_clock() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x52; 32]);
    let other = SecretKey::from_bytes(&[0x53; 32]);
    let fixture = space_fixture(&signer, 0x62)?;
    let state = TempState::new("validator-order")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let scope = AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into());
    let validity = AddressRecordValidity::new(1, NOW_MS + 1, NOW_MS + 600_000)?;
    let envelope = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4102".parse()?)])?,
    )
    .sign(&signer)?;
    let mut invalid_signature = envelope.canonical_bytes().to_vec();
    let last = invalid_signature
        .last_mut()
        .ok_or("signed address record was empty")?;
    *last ^= 1;
    let wrong_target = AddressRecordTarget::new(fixture.genesis.space_id(), other.public().into());

    // When
    let result = AddressRecordValidator::validate_and_store(
        &mut repository,
        &invalid_signature,
        wrong_target.validation(&fixture.authorization, NOW_MS),
    );

    // Then
    assert!(matches!(
        result,
        Err(AddressRecordValidationError::WrongEndpoint)
    ));
    Ok(())
}

#[test]
fn validator_rejects_a_non_member_in_the_requested_space() -> TestResult {
    // Given
    let member = SecretKey::from_bytes(&[0x56; 32]);
    let signer = SecretKey::from_bytes(&[0x57; 32]);
    let fixture = space_fixture(&member, 0x64)?;
    let state = TempState::new("validator-non-member")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let scope = AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into());
    let validity = AddressRecordValidity::new(1, NOW_MS, NOW_MS + 600_000)?;
    let envelope = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4105".parse()?)])?,
    )
    .sign(&signer)?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());

    // When
    let result = AddressRecordValidator::validate_and_store(
        &mut repository,
        envelope.canonical_bytes(),
        target.validation(&fixture.authorization, NOW_MS),
    );

    // Then
    assert!(matches!(
        result,
        Err(AddressRecordValidationError::UnauthorizedMember)
    ));
    Ok(())
}

#[test]
fn validator_rejects_a_future_record_before_persistence() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x58; 32]);
    let fixture = space_fixture(&signer, 0x65)?;
    let state = TempState::new("validator-clock-bounds")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());
    let endpoint_data =
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4106".parse()?)])?;
    let scope = AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into());
    let validity = AddressRecordValidity::new(1, NOW_MS + 1, NOW_MS + 600_000)?;
    let future = SpaceAddressRecordV1::new(scope, validity, endpoint_data).sign(&signer)?;

    // When
    let result = AddressRecordValidator::validate_and_store(
        &mut repository,
        future.canonical_bytes(),
        target.validation(&fixture.authorization, NOW_MS),
    );

    // Then
    assert!(matches!(
        result,
        Err(AddressRecordValidationError::FutureRecord)
    ));
    assert!(
        repository
            .address_record(fixture.genesis.space_id(), signer.public().into())?
            .is_none()
    );
    Ok(())
}

#[test]
fn validator_rejects_an_expired_record_before_persistence() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x5a; 32]);
    let fixture = space_fixture(&signer, 0x67)?;
    let state = TempState::new("validator-expired")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());
    let scope = AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into());
    let validity = AddressRecordValidity::new(1, NOW_MS - 600_000, NOW_MS)?;
    let expired = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4109".parse()?)])?,
    )
    .sign(&signer)?;

    // When
    let result = AddressRecordValidator::validate_and_store(
        &mut repository,
        expired.canonical_bytes(),
        target.validation(&fixture.authorization, NOW_MS),
    );

    // Then
    assert!(matches!(
        result,
        Err(AddressRecordValidationError::ExpiredRecord)
    ));
    Ok(())
}

#[test]
fn validator_rejects_an_invalid_signature_for_the_exact_target() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x59; 32]);
    let fixture = space_fixture(&signer, 0x66)?;
    let state = TempState::new("validator-signature")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let scope = AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into());
    let validity = AddressRecordValidity::new(1, NOW_MS, NOW_MS + 600_000)?;
    let envelope = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4107".parse()?)])?,
    )
    .sign(&signer)?;
    let mut invalid_signature = envelope.canonical_bytes().to_vec();
    let last = invalid_signature
        .last_mut()
        .ok_or("signed address record was empty")?;
    *last ^= 1;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());

    // When
    let result = AddressRecordValidator::validate_and_store(
        &mut repository,
        &invalid_signature,
        target.validation(&fixture.authorization, NOW_MS),
    );

    // Then
    assert!(matches!(
        result,
        Err(AddressRecordValidationError::InvalidSignature)
    ));
    Ok(())
}
