//! Matrix coverage for safe local Iroh relay-map filtering.
#![allow(
    clippy::mod_module_files,
    reason = "integration test support is not an independent test target"
)]

use std::collections::BTreeSet;

use iroh_base::{RelayUrl, SecretKey};
use ma2a_core::{
    MemberCapabilities, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SpaceAuthorizationView, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    AdvertisementValidationContext, LocalIrohRelayMap, PrivateRelayAdvertisementValidator,
    PublicRelayFallbackConfig,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

#[path = "relay_map_matrix/support.rs"]
mod support;
use support::{TempState, TestResult};

const NOW_MS: u64 = 1_700_000_000_000;

#[test]
fn zero_space_endpoint_has_only_explicit_public_fallback() -> TestResult {
    // Given
    let state = TempState::new("relay-map-zero-space")?;
    let repository = Repository::open(&StoreConfig::new(state.path()))?;
    let public_url: RelayUrl = "https://public.example.invalid".parse()?;
    let fallback = PublicRelayFallbackConfig::new(vec![public_url.clone()])?;

    // When
    let map = LocalIrohRelayMap::from_control_spaces(&[], Some(&fallback), NOW_MS);

    // Then
    assert_eq!(map.active_space_count(), 0);
    assert_eq!(map.private_home_relays().count(), 0);
    assert_eq!(map.public_relays(), std::slice::from_ref(&public_url));
    assert!(map.contains(&public_url));
    drop(repository);
    Ok(())
}

#[test]
fn one_space_private_relay_is_both_eligible_and_home_compatible() -> TestResult {
    // Given
    let state = TempState::new("relay-map-one-space")?;
    let config = StoreConfig::new(state.path());
    let local = SecretKey::from_bytes(&[0x11; 32]);
    let relay = SecretKey::from_bytes(&[0x21; 32]);
    let relay_url: RelayUrl = "https://relay-one.example.invalid".parse()?;
    let mut repository = Repository::open(&config)?;
    let authorization = create_space(
        &mut repository,
        SpaceFixture {
            local: &local,
            relays: &[&relay],
            issued_at_ms: 10,
        },
    )?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &authorization,
            relay: &relay,
            relay_url: relay_url.clone(),
            sequence: 1,
        },
    )?;
    let spaces = repository.control_spaces_for(local.public().into())?;

    // When
    let map = LocalIrohRelayMap::from_control_spaces(&spaces, None, NOW_MS);

    // Then
    assert!(map.private_relay_eligible(relay.public().into(), &relay_url));
    assert!(map.home_relay_compatible(relay.public().into(), &relay_url));
    assert!(map.contains(&relay_url));
    Ok(())
}

#[test]
fn multi_space_map_requires_one_provider_to_cover_every_space_even_for_the_same_url() -> TestResult
{
    // Given
    let state = TempState::new("relay-map-many-spaces")?;
    let config = StoreConfig::new(state.path());
    let local = SecretKey::from_bytes(&[0x12; 32]);
    let relay_personal = SecretKey::from_bytes(&[0x31; 32]);
    let relay_lab = SecretKey::from_bytes(&[0x32; 32]);
    let relay_common = SecretKey::from_bytes(&[0x33; 32]);
    let shared_url: RelayUrl = "https://shared.example.invalid".parse()?;
    let common_url: RelayUrl = "https://common.example.invalid".parse()?;
    let public_url: RelayUrl = "https://public.example.invalid".parse()?;
    let fallback = PublicRelayFallbackConfig::new(vec![public_url.clone()])?;
    let mut repository = Repository::open(&config)?;
    let personal = create_space(
        &mut repository,
        SpaceFixture {
            local: &local,
            relays: &[&relay_personal, &relay_common],
            issued_at_ms: 20,
        },
    )?;
    let lab = create_space(
        &mut repository,
        SpaceFixture {
            local: &local,
            relays: &[&relay_lab, &relay_common],
            issued_at_ms: 30,
        },
    )?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &personal,
            relay: &relay_personal,
            relay_url: shared_url.clone(),
            sequence: 1,
        },
    )?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &personal,
            relay: &relay_common,
            relay_url: common_url.clone(),
            sequence: 1,
        },
    )?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &lab,
            relay: &relay_lab,
            relay_url: shared_url.clone(),
            sequence: 1,
        },
    )?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &lab,
            relay: &relay_common,
            relay_url: common_url.clone(),
            sequence: 1,
        },
    )?;
    let spaces = repository.control_spaces_for(local.public().into())?;

    // When
    let map = LocalIrohRelayMap::from_control_spaces(&spaces, Some(&fallback), NOW_MS);

    // Then
    assert!(map.private_relay_eligible(relay_personal.public().into(), &shared_url));
    assert!(map.private_relay_eligible(relay_lab.public().into(), &shared_url));
    assert!(!map.home_relay_compatible(relay_personal.public().into(), &shared_url));
    assert!(!map.home_relay_compatible(relay_lab.public().into(), &shared_url));
    assert!(map.home_relay_compatible(relay_common.public().into(), &common_url));
    assert_eq!(
        map.relay_urls().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([common_url, public_url])
    );
    Ok(())
}

#[test]
fn expired_private_advertisement_is_not_a_candidate() -> TestResult {
    // Given
    let state = TempState::new("relay-map-expired")?;
    let config = StoreConfig::new(state.path());
    let local = SecretKey::from_bytes(&[0x13; 32]);
    let relay = SecretKey::from_bytes(&[0x41; 32]);
    let relay_url: RelayUrl = "https://expired.example.invalid".parse()?;
    let mut repository = Repository::open(&config)?;
    let authorization = create_space(
        &mut repository,
        SpaceFixture {
            local: &local,
            relays: &[&relay],
            issued_at_ms: 40,
        },
    )?;
    store_advertisement(
        &mut repository,
        AdvertisementFixture {
            authorization: &authorization,
            relay: &relay,
            relay_url: relay_url.clone(),
            sequence: 1,
        },
    )?;
    let spaces = repository.control_spaces_for(local.public().into())?;

    // When
    let map = LocalIrohRelayMap::from_control_spaces(&spaces, None, NOW_MS + 600_000);

    // Then
    assert!(!map.private_relay_eligible(relay.public().into(), &relay_url));
    assert!(!map.contains(&relay_url));
    Ok(())
}

#[derive(Clone, Copy)]
struct SpaceFixture<'a> {
    local: &'a SecretKey,
    relays: &'a [&'a SecretKey],
    issued_at_ms: u64,
}

fn create_space(
    repository: &mut Repository,
    fixture: SpaceFixture<'_>,
) -> Result<SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let local_member = SpaceMemberV1::new(
        fixture.local.public().into(),
        format!("local-{}", fixture.issued_at_ms),
        MemberCapabilities::new(true, false),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        fixture.issued_at_ms,
        local_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![local_member];
    for (index, relay) in fixture.relays.iter().enumerate() {
        members.push(SpaceMemberV1::new(
            relay.public().into(),
            format!("relay-{}-{index}", fixture.issued_at_ms),
            MemberCapabilities::new(true, true),
        )?);
    }
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        fixture.issued_at_ms + 1,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    let chain = repository
        .load_space_chain(created.space_id())?
        .ok_or("created Space chain is missing")?;
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

struct AdvertisementFixture<'a> {
    authorization: &'a SpaceAuthorizationView,
    relay: &'a SecretKey,
    relay_url: RelayUrl,
    sequence: u64,
}

fn store_advertisement(
    repository: &mut Repository,
    fixture: AdvertisementFixture<'_>,
) -> TestResult {
    let signed = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(
            fixture.authorization.space_id(),
            fixture.relay.public().into(),
        ),
        fixture.relay_url,
        PrivateRelayAdvertisementValidity::new(fixture.sequence, NOW_MS, NOW_MS + 600_000)?,
    )?
    .sign(fixture.relay)?;
    PrivateRelayAdvertisementValidator::validate_and_store(
        repository,
        signed.canonical_bytes(),
        AdvertisementValidationContext::new(fixture.authorization, NOW_MS),
    )?;
    Ok(())
}
