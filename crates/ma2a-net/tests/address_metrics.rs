//! Typed low-cardinality address metrics coverage.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1,
};
use ma2a_net::{
    AddressMetrics, AddressPersistenceOutcome, AddressRecordTarget, AddressRecordValidationError,
    AddressRecordValidator, AddressValidationOutcome,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[derive(Clone, Copy)]
struct SignedInput<'a> {
    signer: &'a SecretKey,
    space_id: ma2a_core::SpaceId,
    sequence: u64,
    port: u16,
}

#[test]
fn validator_metrics_count_accepted_and_persistence_outcomes() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0xa1; 32]);
    let fixture = space_fixture(&signer, 0xa2)?;
    let state = TempState::new("address-metrics-validation")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let envelope = SpaceAddressRecordV1::new(
        AddressRecordScope::new(fixture.genesis.space_id(), signer.public().into()),
        AddressRecordValidity::new(1, NOW_MS, NOW_MS + 600_000)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4501".parse()?)])?,
    )
    .sign(&signer)?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());
    let metrics = AddressMetrics::default();

    // When
    AddressRecordValidator::validate_and_store(
        &mut repository,
        envelope.canonical_bytes(),
        target
            .validation(&fixture.authorization, NOW_MS)
            .with_metrics(&metrics),
    )?;
    AddressRecordValidator::validate_and_store(
        &mut repository,
        envelope.canonical_bytes(),
        target
            .validation(&fixture.authorization, NOW_MS)
            .with_metrics(&metrics),
    )?;
    let snapshot = metrics.snapshot();

    // Then
    assert_eq!(snapshot.validation(AddressValidationOutcome::Accepted), 2);
    assert_eq!(snapshot.persistence(AddressPersistenceOutcome::Advanced), 1);
    assert_eq!(
        snapshot.persistence(AddressPersistenceOutcome::Idempotent),
        1
    );
    Ok(())
}

#[test]
fn validator_metrics_count_rollback_and_fork_outcomes() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0xa3; 32]);
    let fixture = space_fixture(&signer, 0xa4)?;
    let state = TempState::new("address-metrics-sequences")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let target = AddressRecordTarget::new(fixture.genesis.space_id(), signer.public().into());
    let metrics = AddressMetrics::default();
    let accepted = signed(SignedInput {
        signer: &signer,
        space_id: fixture.genesis.space_id(),
        sequence: 2,
        port: 4502,
    })?;
    AddressRecordValidator::validate_and_store(
        &mut repository,
        accepted.canonical_bytes(),
        target
            .validation(&fixture.authorization, NOW_MS)
            .with_metrics(&metrics),
    )?;
    let rollback = signed(SignedInput {
        signer: &signer,
        space_id: fixture.genesis.space_id(),
        sequence: 1,
        port: 4503,
    })?;
    let fork = signed(SignedInput {
        signer: &signer,
        space_id: fixture.genesis.space_id(),
        sequence: 2,
        port: 4504,
    })?;

    // When
    let rollback_result = AddressRecordValidator::validate_and_store(
        &mut repository,
        rollback.canonical_bytes(),
        target
            .validation(&fixture.authorization, NOW_MS)
            .with_metrics(&metrics),
    );
    let fork_result = AddressRecordValidator::validate_and_store(
        &mut repository,
        fork.canonical_bytes(),
        target
            .validation(&fixture.authorization, NOW_MS)
            .with_metrics(&metrics),
    );
    let snapshot = metrics.snapshot();

    // Then
    assert!(matches!(
        rollback_result,
        Err(AddressRecordValidationError::Rollback)
    ));
    assert!(matches!(
        fork_result,
        Err(AddressRecordValidationError::Fork)
    ));
    assert_eq!(snapshot.persistence(AddressPersistenceOutcome::Rollback), 1);
    assert_eq!(snapshot.persistence(AddressPersistenceOutcome::Fork), 1);
    assert_eq!(snapshot.validation(AddressValidationOutcome::Rollback), 1);
    assert_eq!(snapshot.validation(AddressValidationOutcome::Fork), 1);
    Ok(())
}

fn signed(
    input: SignedInput<'_>,
) -> Result<ma2a_core::SignedSpaceAddressRecordV1, Box<dyn std::error::Error + Send + Sync>> {
    Ok(SpaceAddressRecordV1::new(
        AddressRecordScope::new(input.space_id, input.signer.public().into()),
        AddressRecordValidity::new(input.sequence, NOW_MS, NOW_MS + 600_000)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip(
            format!("127.0.0.1:{}", input.port).parse()?,
        )])?,
    )
    .sign(input.signer)?)
}
