//! Complete target-only endpoint data lookup coverage.
#![allow(
    clippy::mod_module_files,
    reason = "shared integration test support is not an independent test target"
)]

mod support;

use std::sync::Arc;

use iroh_base::{CustomAddr, SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, SpaceAddressRecordV1,
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

struct LookupRecordInput<'a> {
    signer: &'a SecretKey,
    fixture: &'a support::SpaceFixture,
    addresses: Vec<TransportAddr>,
    user_data: Option<&'a str>,
}

#[test]
fn lookup_reconstructs_ordered_custom_addresses_and_user_data() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0xb1; 32]);
    let first_space = space_fixture(&signer, 0xb2)?;
    let second_space = space_fixture(&signer, 0xb3)?;
    let state = TempState::new("lookup-complete-data")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    for fixture in [&first_space, &second_space] {
        repository.create_space(&SpaceRecord::new(
            fixture.genesis.space_id(),
            fixture.genesis.canonical_bytes().to_vec(),
        ))?;
    }
    let shared = TransportAddr::Custom(CustomAddr::from_parts(7, b"shared"));
    let first = validate(
        &mut repository,
        LookupRecordInput {
            signer: &signer,
            fixture: &first_space,
            addresses: vec![
                TransportAddr::Relay("https://relay.example.test".parse()?),
                shared.clone(),
            ],
            user_data: Some("metadata"),
        },
    )?;
    let second = validate(
        &mut repository,
        LookupRecordInput {
            signer: &signer,
            fixture: &second_space,
            addresses: vec![
                shared,
                TransportAddr::Ip("127.0.0.1:4601".parse()?),
                TransportAddr::Ip("[::1]:4602".parse()?),
            ],
            user_data: Some("metadata"),
        },
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(vec![
        first_space.authorization.clone(),
        second_space.authorization.clone(),
    ])?;
    lookup.cache(first)?;
    lookup.cache(second)?;

    // When
    let resolved = lookup
        .resolve_endpoint(signer.public())
        .ok_or("complete lookup returned no result")?;
    let addresses = resolved
        .data
        .addrs()
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    // Then
    assert_eq!(
        addresses,
        vec![
            "relay:https://relay.example.test/",
            "custom:7_736861726564",
            "ip:127.0.0.1:4601",
            "ip:[::1]:4602",
        ]
    );
    assert_eq!(
        resolved.data.user_data().map(ToString::to_string),
        Some("metadata".to_owned())
    );
    Ok(())
}

#[test]
fn lookup_returns_empty_when_spaces_disagree_on_user_data() -> TestResult {
    // Given
    let signer = SecretKey::from_bytes(&[0xb4; 32]);
    let first_space = space_fixture(&signer, 0xb5)?;
    let second_space = space_fixture(&signer, 0xb6)?;
    let state = TempState::new("lookup-user-data-conflict")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    for fixture in [&first_space, &second_space] {
        repository.create_space(&SpaceRecord::new(
            fixture.genesis.space_id(),
            fixture.genesis.canonical_bytes().to_vec(),
        ))?;
    }
    let first = validate(
        &mut repository,
        LookupRecordInput {
            signer: &signer,
            fixture: &first_space,
            addresses: vec![TransportAddr::Ip("127.0.0.1:4603".parse()?)],
            user_data: None,
        },
    )?;
    let second = validate(
        &mut repository,
        LookupRecordInput {
            signer: &signer,
            fixture: &second_space,
            addresses: vec![TransportAddr::Ip("127.0.0.1:4604".parse()?)],
            user_data: Some(""),
        },
    )?;
    let lookup = SpaceAddressLookup::with_clock(Arc::new(FixedClock));
    lookup.replace_authorizations(vec![
        first_space.authorization.clone(),
        second_space.authorization.clone(),
    ])?;
    lookup.cache(first)?;
    lookup.cache(second)?;

    // When
    let resolved = lookup.resolve_endpoint(signer.public());

    // Then
    assert!(resolved.is_none());
    Ok(())
}

fn validate(
    repository: &mut Repository,
    input: LookupRecordInput<'_>,
) -> Result<ma2a_net::ValidatedAddressRecord, Box<dyn std::error::Error + Send + Sync>> {
    let envelope = SpaceAddressRecordV1::new(
        AddressRecordScope::new(
            input.fixture.genesis.space_id(),
            input.signer.public().into(),
        ),
        AddressRecordValidity::new(1, NOW_MS, NOW_MS + 600_000)?,
        AddressEndpointDataV1::from_parts(input.addresses, input.user_data.map(str::to_owned))?,
    )
    .sign(input.signer)?;
    Ok(AddressRecordValidator::validate_and_store(
        repository,
        envelope.canonical_bytes(),
        AddressRecordTarget::new(
            input.fixture.genesis.space_id(),
            input.signer.public().into(),
        )
        .validation(&input.fixture.authorization, NOW_MS),
    )?)
}
