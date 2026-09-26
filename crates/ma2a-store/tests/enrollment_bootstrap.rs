//! Atomic enrollment bootstrap persistence coverage.

#[path = "common/support.rs"]
mod support;

use iroh_base::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity, EnrollmentBootstrap,
    MemberCapabilities, SpaceAddressRecordV1, SpaceAuthoritySecret, SpaceAuthorizationView,
    SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceManifestLink,
    SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{
    AddressRecordBoundaryError, AddressRecordTarget, AddressRecordValidation, ControlBatch,
    Repository, StoreConfig, ValidatedAddressRecord,
};
use support::{TempState, TestResult};

const NOW_MS: u64 = 2_000;

#[test]
fn valid_bootstrap_persists_chain_and_owner_address_at_one_revision() -> TestResult {
    // Given
    let state = TempState::new("valid-enrollment-bootstrap")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let fixture = fixture()?;
    let frames = EnrollmentBootstrap::new(fixture.chain.clone(), fixture.owner_record.clone())?
        .encode_frames()?;
    let bootstrap = EnrollmentBootstrap::decode_frames(&frames)?;
    let authorization = SpaceAuthorizationView::from_chain(bootstrap.chain());
    let validated = ValidatedAddressRecord::parse(
        bootstrap.owner_address_record().canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(bootstrap.chain().space_id(), fixture.owner.public().into()),
            &authorization,
            NOW_MS,
        ),
    )?;

    // When
    let revision = repository.persist_control_batch(&ControlBatch::new(
        vec![bootstrap.chain().clone()],
        vec![validated],
        Vec::new(),
    ))?;

    // Then
    assert_eq!(revision, Some(1));
    assert_eq!(
        repository.load_space_chain(fixture.chain.space_id())?,
        Some(fixture.chain)
    );
    assert_eq!(
        repository
            .address_record(bootstrap.chain().space_id(), fixture.owner.public().into(),)?
            .ok_or("owner address missing")?
            .signed_record(),
        fixture.owner_record.canonical_bytes()
    );
    Ok(())
}

#[test]
fn wrong_owner_and_wrong_space_bootstraps_leave_no_partial_state() -> TestResult {
    for (label, target, expected) in [
        (
            "wrong-owner",
            Target::Owner(SecretKey::from_bytes(&[0x55; 32])),
            AddressRecordBoundaryError::WrongEndpoint,
        ),
        (
            "wrong-space",
            Target::Space(ma2a_core::SpaceId::derive(b"wrong-enrollment-space")),
            AddressRecordBoundaryError::WrongSpace,
        ),
    ] {
        // Given
        let state = TempState::new(label)?;
        let repository = Repository::open(&StoreConfig::new(state.path()))?;
        let fixture = fixture()?;
        let authorization = SpaceAuthorizationView::from_chain(&fixture.chain);
        let target = match target {
            Target::Owner(secret) => {
                AddressRecordTarget::new(fixture.chain.space_id(), secret.public().into())
            }
            Target::Space(space_id) => {
                AddressRecordTarget::new(space_id, fixture.owner.public().into())
            }
        };

        // When
        let result = ValidatedAddressRecord::parse(
            fixture.owner_record.canonical_bytes(),
            AddressRecordValidation::new(target, &authorization, NOW_MS),
        );

        // Then
        assert_eq!(result, Err(expected));
        assert_eq!(repository.revision()?, 0);
        assert_eq!(repository.load_space_chain(fixture.chain.space_id())?, None);
        assert_eq!(
            repository.address_record(fixture.chain.space_id(), fixture.owner.public().into())?,
            None
        );
    }
    Ok(())
}

enum Target {
    Owner(SecretKey),
    Space(ma2a_core::SpaceId),
}

struct Fixture {
    chain: SpaceChain,
    owner: SecretKey,
    owner_record: ma2a_core::SignedSpaceAddressRecordV1,
}

fn fixture() -> Result<Fixture, Box<dyn std::error::Error + Send + Sync>> {
    let authority = SpaceAuthoritySecret::from_bytes([0x41; 32]);
    let owner = SecretKey::from_bytes(&[0x42; 32]);
    let candidate = SecretKey::from_bytes(&[0x43; 32]);
    let owner_member = member(&owner, "owner")?;
    let candidate_member = member(&candidate, "candidate")?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x44; 32], 1_000, authority.public_key())?,
        SpaceGenesisOwner::new(owner_member.clone(), SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    let mut members = vec![owner_member, candidate_member];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let manifest = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        NOW_MS,
        SpaceManifestMembership::new(members, Vec::new()),
    )?
    .sign(&authority)?;
    chain.apply(&manifest)?;
    let owner_record = SpaceAddressRecordV1::new(
        AddressRecordScope::new(chain.space_id(), owner.public().into()),
        AddressRecordValidity::new(1, 1_000, 301_000)?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip("127.0.0.1:4242".parse()?)])?,
    )
    .sign(&owner)?;
    Ok(Fixture {
        chain,
        owner,
        owner_record,
    })
}

fn member(secret: &SecretKey, label: &str) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    SpaceMemberV1::new(
        secret.public().into(),
        label.to_owned(),
        MemberCapabilities::new(true, false),
    )
}

#[test]
fn bootstrap_receipt_and_noop_retry_use_the_transaction_projection() -> TestResult {
    let state = TempState::new("enrollment-receipt")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let fixture = fixture()?;
    let space = fixture.chain.space_id();
    let endpoint = fixture.owner.public().into();
    let authorization = SpaceAuthorizationView::from_chain(&fixture.chain);
    let validated = ValidatedAddressRecord::parse(
        fixture.owner_record.canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(space, endpoint),
            &authorization,
            NOW_MS,
        ),
    )?;
    let batch = ControlBatch::new(vec![fixture.chain.clone()], vec![validated], vec![]);
    let committed = repository.persist_enrollment_batch(&batch, space, endpoint)?;
    assert_eq!(committed.revision(), repository.revision()?);
    assert_eq!(committed.value().chain, fixture.chain);
    assert_eq!(
        committed.value().memberships,
        repository.memberships_for(endpoint)?
    );
    let retry = repository.persist_enrollment_batch(&batch, space, endpoint)?;
    assert_eq!(retry.revision(), committed.revision());
    assert_eq!(retry.value().chain, committed.value().chain);
    let mut other = Repository::open(&config)?;
    other.advance_revision()?;
    assert_eq!(committed.revision() + 1, other.revision()?);
    assert_eq!(committed.value().chain, fixture.chain);
    Ok(())
}
