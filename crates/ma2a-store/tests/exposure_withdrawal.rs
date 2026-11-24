//! Consent to serve a Space ends with this Endpoint's membership of it.

#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    MemberCapabilities, SpaceAuthoritySecret, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner,
    SpaceGenesisV1, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1,
    SpacePolicyV1, SpaceRevocationV1,
    service::{RootAccess, RootName},
};
use ma2a_store::{
    EndpointRecord, FileRootExposure, KeyKind, KeyMaterial, KeyReference, KeyStore, LocalExposure,
    Repository, StoreConfig, StoreError,
};
use support::{TempState, TestResult, TestResultValue as TestValue};

/// Public keys whose Endpoint IDs are valid curve points.
const OWNER: [u8; 32] = [
    0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba, 0xa0, 0x1f,
    0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3, 0x7f, 0xcc, 0x1f, 0x9d,
];
const LOCAL: [u8; 32] = [
    0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e, 0xab, 0x6c,
    0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06, 0x98, 0x81, 0xdb, 0x12,
];

fn member(bytes: [u8; 32], owner: bool) -> TestValue<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        ma2a_core::EndpointId::try_from(bytes.as_slice())?,
        if owner { "owner" } else { "local" }.to_owned(),
        MemberCapabilities::new(true, owner),
    )?)
}

/// Signs one Space with `seed`, then a generation adding the local Endpoint.
fn joined(seed: u8) -> TestValue<(SpaceChain, SpaceAuthoritySecret)> {
    let secret = SpaceAuthoritySecret::from_bytes([seed; 32]);
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([seed ^ 0x0f; 32], 10, secret.public_key())?,
        SpaceGenesisOwner::new(member(OWNER, true)?, SpacePolicyV1::phase_one_default()),
    )
    .sign(&secret)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    // Endpoint IDs sort by bytes, and LOCAL sorts before OWNER.
    let members = vec![member(LOCAL, false)?, member(OWNER, true)?];
    append(&mut chain, &secret, (members, vec![]))?;
    Ok((chain, secret))
}

fn append(
    chain: &mut SpaceChain,
    secret: &SpaceAuthoritySecret,
    (members, revocations): (Vec<SpaceMemberV1>, Vec<SpaceRevocationV1>),
) -> TestResult {
    let generation = chain.latest_generation() + 1;
    let manifest = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), generation, chain.latest_hash()),
        10 + generation,
        SpaceManifestMembership::new(members, revocations),
    )?
    .sign(secret)?;
    chain.apply(&manifest)?;
    Ok(())
}

fn exposed_spaces(repository: &Repository) -> TestValue<Vec<ma2a_core::SpaceId>> {
    Ok(repository
        .service_exposures()?
        .into_value()
        .iter()
        .map(|record| record.space_id)
        .collect())
}

/// Opens a Repository whose Endpoint identity is `LOCAL`.
fn local_repository(state: &TempState) -> TestValue<Repository> {
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let key = KeyReference::parse("endpoint-main")?;
    // These tests never sign; the protected key only has to exist to record the identity.
    KeyStore::open(state.path())?.write(KeyMaterial::new(KeyKind::Endpoint, &key, &[0x5a; 32]))?;
    repository.set_endpoint(&EndpointRecord::new(
        ma2a_core::EndpointId::try_from(LOCAL.as_slice())?,
        key,
    ))?;
    Ok(repository)
}

#[test]
fn leaving_a_space_withdraws_its_exposures_and_keeps_every_other() -> TestResult {
    let state = TempState::new("exposure-withdrawal")?;
    let mut repository = local_repository(&state)?;
    let (mut left, secret) = joined(0x71)?;
    let (kept, _) = joined(0x72)?;
    repository.import_space_chain(&left.export_public()?)?;
    repository.import_space_chain(&kept.export_public()?)?;
    let docs = LocalExposure::FilesRoot(FileRootExposure::new(
        RootName::new("docs")?,
        "/srv/docs".to_owned(),
        RootAccess::ReadWrite,
    ));
    repository.expose_service(left.space_id(), &docs)?;
    repository.expose_service(left.space_id(), &LocalExposure::Exec)?;
    repository.expose_service(kept.space_id(), &LocalExposure::Shell)?;

    // A later generation without this Endpoint arrives, as after leaving or removal.
    let departed = SpaceRevocationV1::new(ma2a_core::EndpointId::try_from(LOCAL.as_slice())?);
    append(
        &mut left,
        &secret,
        (vec![member(OWNER, true)?], vec![departed]),
    )?;
    repository.import_space_chain(&left.export_public()?)?;

    assert_eq!(exposed_spaces(&repository)?, vec![kept.space_id()]);
    // The root name bound only by the withdrawn exposure is forgotten with it.
    let rebound = LocalExposure::FilesRoot(FileRootExposure::new(
        RootName::new("docs")?,
        "/elsewhere".to_owned(),
        RootAccess::ReadOnly,
    ));
    repository.expose_service(kept.space_id(), &rebound)?;
    Ok(())
}

#[test]
fn a_space_this_endpoint_left_cannot_be_exposed() -> TestResult {
    // Given: this Endpoint was removed from a Space it still has on record.
    let state = TempState::new("exposure-after-departure")?;
    let mut repository = local_repository(&state)?;
    let (mut left, secret) = joined(0x73)?;
    let departed = SpaceRevocationV1::new(ma2a_core::EndpointId::try_from(LOCAL.as_slice())?);
    append(
        &mut left,
        &secret,
        (vec![member(OWNER, true)?], vec![departed]),
    )?;
    repository.import_space_chain(&left.export_public()?)?;

    // When
    let exposed = repository.expose_service(left.space_id(), &LocalExposure::Exec);

    // Then: no consent is recorded that a later rejoin would silently revive.
    assert!(
        matches!(exposed, Err(StoreError::SpaceNotFound)),
        "{exposed:?}"
    );
    assert!(exposed_spaces(&repository)?.is_empty());
    Ok(())
}
