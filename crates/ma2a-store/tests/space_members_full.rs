//! A locally owned Space with no member slot left refuses an enrollment before signing it, and
//! the invitation stays unused.
#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    EndpointId, InviteEntropy, InviteValidity, MemberCapabilities, RequestId, SpaceAuthoritySecret,
    SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, EnrollmentRedemption, OwnedSpaceUpdate,
    Repository, SpaceCreation, StoreConfig, StoreError,
};
use support::{TempState, TestResult, TestResultValue};

fn member(seed: u8, owner: bool) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    let key = SpaceAuthoritySecret::from_bytes([seed; 32]).public_key();
    SpaceMemberV1::new(
        EndpointId::try_from(key.as_bytes().as_slice())?,
        format!("member-{seed}"),
        MemberCapabilities::new(true, owner),
    )
}

fn redemption(
    invite: &ma2a_store::CreatedEnrollmentInvite,
    (seed, request): (u8, u8),
) -> TestResultValue<AuthorizedEnrollmentRedemption> {
    Ok(AuthorizedEnrollmentRedemption::new(
        invite.ticket().clone(),
        EnrollmentRedemption {
            endpoint_id: member(seed, false)?.endpoint_id(),
            request_id: RequestId::try_from([request; 16].as_slice())?,
            display_name: format!("candidate-{seed}"),
        },
        3_000,
    ))
}

#[test]
fn the_last_slot_is_taken_and_the_next_invitation_is_refused_unused() -> TestResult {
    // Given: an owned Space whose default policy allows 64 members, holding 63.
    let state = TempState::new("space-members-full")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let owner = member(200, true)?;
    let policy = SpacePolicyV1::phase_one_default();
    let space_id = repository
        .create_owned_space(&SpaceCreation::new(1_000, owner.clone(), policy))?
        .space_id();
    let mut members = (1..=62)
        .map(|seed| member(seed, false))
        .collect::<Result<Vec<_>, _>>()?;
    members.push(owner.clone());
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    assert_eq!(members.len() + 1, policy.maximum_members());
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        2_000,
        SpaceManifestMembership::new(members, vec![]),
    ))?;
    let address = iroh_base::EndpointAddr::new(iroh_base::PublicKey::from_bytes(
        owner.endpoint_id().as_bytes(),
    )?);
    let invite = |repository: &mut Repository| -> TestResultValue<_> {
        Ok(repository.create_enrollment_invite(
            space_id,
            owner.endpoint_id(),
            address.clone(),
            InviteValidity::new(2_500, 20_000)?,
            &InviteEntropy::random()?,
        )?)
    };
    let (last, next) = (invite(&mut repository)?, invite(&mut repository)?);

    // When / Then: the first invitation takes the last slot.
    assert!(matches!(
        repository.redeem_enrollment(&redemption(&last, (100, 1))?)?,
        EnrollmentOutcome::Redeemed { .. }
    ));
    let revision = repository.revision()?;

    // When / Then: the next one is refused, commits nothing, and is refused the same way when
    // asked again with another request, so it was never consumed.
    for request in [2, 3] {
        assert!(matches!(
            repository.redeem_enrollment(&redemption(&next, (101, request))?),
            Err(StoreError::SpaceMembersFull)
        ));
    }
    assert_eq!(repository.revision()?, revision);
    Ok(())
}

#[test]
fn revoked_or_expired_consumed_enrollment_cannot_replay() -> TestResult {
    use ma2a_core::SpaceRevocationV1;

    // Given: an owned Space where one candidate redeemed an invitation valid until 20_000 ms.
    let state = TempState::new("enrollment-retry-revoked")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let owner = member(201, true)?;
    let policy = SpacePolicyV1::phase_one_default();
    let space_id = repository
        .create_owned_space(&SpaceCreation::new(1_000, owner.clone(), policy))?
        .space_id();
    let address = iroh_base::EndpointAddr::new(iroh_base::PublicKey::from_bytes(
        owner.endpoint_id().as_bytes(),
    )?);
    let invite = repository.create_enrollment_invite(
        space_id,
        owner.endpoint_id(),
        address,
        InviteValidity::new(2_500, 20_000)?,
        &InviteEntropy::random()?,
    )?;
    let attempt = |now_ms| -> TestResultValue<AuthorizedEnrollmentRedemption> {
        Ok(AuthorizedEnrollmentRedemption::new(
            invite.ticket().clone(),
            EnrollmentRedemption {
                endpoint_id: member(55, false)?.endpoint_id(),
                request_id: RequestId::try_from([9; 16].as_slice())?,
                display_name: "candidate-55".to_owned(),
            },
            now_ms,
        ))
    };
    assert!(matches!(
        repository.redeem_enrollment(&attempt(3_000)?)?,
        EnrollmentOutcome::Redeemed { .. }
    ));

    // While unexpired and still a member, retrying returns Retry.
    assert!(matches!(
        repository.redeem_enrollment(&attempt(4_000)?)?,
        EnrollmentOutcome::Retry { .. }
    ));

    // Past the invitation's expiry, replaying returns Expired.
    assert!(matches!(
        repository.redeem_enrollment(&attempt(25_000)?)?,
        EnrollmentOutcome::Expired
    ));

    // After the member is revoked, even an unexpired retry is refused with Conflict.
    let candidate_id = member(55, false)?.endpoint_id();
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        space_id,
        5_000,
        SpaceManifestMembership::new(vec![owner], vec![SpaceRevocationV1::new(candidate_id)]),
    ))?;
    assert!(matches!(
        repository.redeem_enrollment(&attempt(6_000)?)?,
        EnrollmentOutcome::Conflict
    ));
    Ok(())
}
