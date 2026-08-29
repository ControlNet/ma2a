//! Adversarial monotonicity and uncertainty coverage for private address lookup.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, MemberCapabilities,
    SpaceAddressRecordV1, SpaceAuthoritySecret, SpaceAuthorizationView, SpaceChain,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpaceRevocationV1,
};
use ma2a_net::{
    AddressLookupClock, AddressRecordTarget, AddressRecordValidator, SpaceAddressLookup,
    ValidatedAddressRecord,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[derive(Debug)]
struct ReviewClock(AtomicU64);

impl ReviewClock {
    const fn new(now_ms: u64) -> Self {
        Self(AtomicU64::new(now_ms))
    }

    fn set(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::Relaxed);
    }
}

impl AddressLookupClock for ReviewClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[test]
fn lookup_does_not_regress_when_an_older_validated_record_arrives_late() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x81; 32]);
    let fixture = space_fixture(&signer, 0x82)?;
    let state = TempState::new("lookup-cache-rollback")?;
    let mut repository = repository(&state, &fixture)?;
    let older = validate(
        &mut repository,
        &RecordInput {
            signer: &signer,
            fixture: &fixture,
            sequence: 1,
            address: "127.0.0.1:4301",
        },
    )?;
    let newer = validate(
        &mut repository,
        &RecordInput {
            signer: &signer,
            fixture: &fixture,
            sequence: 2,
            address: "127.0.0.1:4302",
        },
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(ReviewClock::new(NOW_MS)));
    lookup.replace_authorizations(vec![fixture.authorization.clone()])?;
    lookup.cache(newer)?;

    // When
    lookup.cache(older)?;
    let resolved = lookup
        .resolve_endpoint(signer.public())
        .ok_or("newer cached address disappeared")?;
    let addresses = resolved
        .data
        .addrs()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Then
    assert_eq!(addresses, vec!["ip:127.0.0.1:4302"]);
    Ok(())
}

#[test]
fn lookup_returns_empty_when_clock_rollback_makes_a_record_future_dated() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x83; 32]);
    let fixture = space_fixture(&signer, 0x84)?;
    let state = TempState::new("lookup-clock-rollback")?;
    let mut repository = repository(&state, &fixture)?;
    let record = validate(
        &mut repository,
        &RecordInput {
            signer: &signer,
            fixture: &fixture,
            sequence: 1,
            address: "127.0.0.1:4303",
        },
    )?;
    let clock = Arc::new(ReviewClock::new(NOW_MS));
    let lookup_clock = Arc::clone(&clock);
    let lookup = SpaceAddressLookup::with_clock(lookup_clock);
    lookup.replace_authorizations(vec![fixture.authorization.clone()])?;
    lookup.cache(record)?;

    // When
    clock.set(NOW_MS - 1);
    let resolved = lookup.resolve_endpoint(signer.public());

    // Then
    assert!(resolved.is_none());
    Ok(())
}

#[test]
fn lookup_rejects_conflicting_authorization_views_for_one_space() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0x85; 32]);
    let replacement = SecretKey::from_bytes(&[0x86; 32]);
    let fixture = space_fixture(&signer, 0x87)?;
    let revoked = revoked_authorization(&fixture, &replacement, 0x87)?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(ReviewClock::new(NOW_MS)));

    // When
    let result = lookup.replace_authorizations(vec![fixture.authorization, revoked]);

    // Then
    assert!(result.is_err());
    Ok(())
}

fn repository(
    state: &TempState,
    fixture: &support::SpaceFixture,
) -> Result<Repository, Box<dyn std::error::Error + Send + Sync>> {
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    Ok(repository)
}

struct RecordInput<'a> {
    signer: &'a SecretKey,
    fixture: &'a support::SpaceFixture,
    sequence: u64,
    address: &'a str,
}

fn validate(
    repository: &mut Repository,
    input: &RecordInput<'_>,
) -> Result<ValidatedAddressRecord, Box<dyn std::error::Error + Send + Sync>> {
    let scope = AddressRecordScope::new(
        input.fixture.genesis.space_id(),
        input.signer.public().into(),
    );
    let validity = AddressRecordValidity::new(input.sequence, NOW_MS, NOW_MS + 600_000)?;
    let envelope = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip(input.address.parse()?)])?,
    )
    .sign(input.signer)?;
    let target = AddressRecordTarget::new(
        input.fixture.genesis.space_id(),
        input.signer.public().into(),
    );
    Ok(AddressRecordValidator::validate_and_store(
        repository,
        envelope.canonical_bytes(),
        target.validation(&input.fixture.authorization, NOW_MS),
    )?)
}

fn revoked_authorization(
    fixture: &support::SpaceFixture,
    replacement: &SecretKey,
    authority_marker: u8,
) -> Result<SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let authority = SpaceAuthoritySecret::from_bytes([authority_marker; 32]);
    let replacement = SpaceMemberV1::new(
        replacement.public().into(),
        "replacement".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let revocation =
        SpaceRevocationV1::new(fixture.genesis.genesis().initial_member().endpoint_id());
    let manifest = SpaceManifestV1::new(
        SpaceManifestLink::new(fixture.genesis.space_id(), 1, fixture.genesis.chain_hash()),
        2,
        SpaceManifestMembership::new(vec![replacement], vec![revocation]),
    )?
    .sign(&authority)?;
    let mut chain = SpaceChain::from_genesis(fixture.genesis.clone())?;
    chain.apply(&manifest)?;
    Ok(SpaceAuthorizationView::from_chain(&chain))
}
