//! Exact-target multi-Space address lookup coverage.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use std::sync::Arc;

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, MemberCapabilities,
    SpaceAddressRecordV1, SpaceAuthoritySecret, SpaceAuthorizationView, SpaceChain,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpaceRevocationV1,
};
use ma2a_net::{
    AddressLookupClock, AddressRecordTarget, AddressRecordValidator, SpaceAddressLookup,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};
use support::{TempState, TestResult, space_fixture};

const NOW_MS: u64 = 1_700_000_000_000;

#[derive(Debug)]
struct FixedClock;

impl AddressLookupClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

#[test]
fn lookup_merges_only_requested_endpoint_records_across_spaces() -> TestResult {
    // Given
    let target_secret = SecretKey::from_bytes(&[0x71; 32]);
    let unrelated_secret = SecretKey::from_bytes(&[0x72; 32]);
    let first_space = space_fixture(&target_secret, 0x73)?;
    let second_space = space_fixture(&target_secret, 0x74)?;
    let unrelated_space = space_fixture(&unrelated_secret, 0x75)?;
    let state = TempState::new("target-only-lookup")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let fixtures = [&first_space, &second_space, &unrelated_space];
    for fixture in fixtures {
        repository.create_space(&SpaceRecord::new(
            fixture.genesis.space_id(),
            fixture.genesis.canonical_bytes().to_vec(),
        ))?;
    }
    let first = validate(
        &mut repository,
        AddressFixture::new(&target_secret, &first_space, "127.0.0.1:4201"),
    )?;
    let second = validate(
        &mut repository,
        AddressFixture::new(&target_secret, &second_space, "127.0.0.1:4202"),
    )?;
    let unrelated = validate(
        &mut repository,
        AddressFixture::new(&unrelated_secret, &unrelated_space, "127.0.0.1:4299"),
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(vec![
        first_space.authorization.clone(),
        second_space.authorization.clone(),
        unrelated_space.authorization.clone(),
    ])?;
    lookup.cache(first)?;
    lookup.cache(second)?;
    lookup.cache(unrelated)?;

    // When
    let resolved = lookup
        .resolve_endpoint(target_secret.public())
        .ok_or("target lookup returned no result")?;
    let addresses = resolved
        .data
        .addrs()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Then
    assert_eq!(resolved.endpoint_id, target_secret.public());
    assert_eq!(addresses, vec!["ip:127.0.0.1:4201", "ip:127.0.0.1:4202"]);
    assert!(!addresses.iter().any(|address| address.ends_with(":4299")));
    Ok(())
}

#[test]
fn lookup_removes_a_record_after_space_local_revocation() -> TestResult {
    // Given
    let target_secret = SecretKey::from_bytes(&[0x76; 32]);
    let replacement = SecretKey::from_bytes(&[0x77; 32]);
    let fixture = space_fixture(&target_secret, 0x78)?;
    let revoked = revoked_authorization(&fixture, &replacement, 0x78)?;
    let state = TempState::new("revoked-target-lookup")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        fixture.genesis.space_id(),
        fixture.genesis.canonical_bytes().to_vec(),
    ))?;
    let record = validate(
        &mut repository,
        AddressFixture::new(&target_secret, &fixture, "127.0.0.1:4210"),
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(vec![revoked])?;
    lookup.cache(record)?;

    // When
    let resolved = lookup.resolve_endpoint(target_secret.public());

    // Then
    assert!(resolved.is_none());
    Ok(())
}

#[derive(Clone, Copy)]
struct AddressFixture<'a> {
    secret: &'a SecretKey,
    space: &'a support::SpaceFixture,
    address: &'a str,
}

impl<'a> AddressFixture<'a> {
    const fn new(
        secret: &'a SecretKey,
        space: &'a support::SpaceFixture,
        address: &'a str,
    ) -> Self {
        Self {
            secret,
            space,
            address,
        }
    }
}

fn validate(
    repository: &mut Repository,
    fixture: AddressFixture<'_>,
) -> Result<ma2a_net::ValidatedAddressRecord, Box<dyn std::error::Error + Send + Sync>> {
    let scope = AddressRecordScope::new(
        fixture.space.genesis.space_id(),
        fixture.secret.public().into(),
    );
    let validity = AddressRecordValidity::new(1, NOW_MS - 1, NOW_MS + 599_999)?;
    let envelope = SpaceAddressRecordV1::new(
        scope,
        validity,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip(fixture.address.parse()?)])?,
    )
    .sign(fixture.secret)?;
    let target = AddressRecordTarget::new(
        fixture.space.genesis.space_id(),
        fixture.secret.public().into(),
    );
    Ok(AddressRecordValidator::validate_and_store(
        repository,
        envelope.canonical_bytes(),
        target.validation(&fixture.space.authorization, NOW_MS),
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
