//! A departure sent to the Endpoint that created a Space but no longer holds its authority key
//! is refused, and that Endpoint keeps running.
use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, SpaceDepartureErrorCode};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// Far below the thirty-second exchange bound, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_creator_without_the_authority_key_refuses_a_departure_and_keeps_running() -> TestResult {
    // Given: a Space with one member, whose creator has since lost its authority key, as a
    // Runtime that imported the public chain under the same Endpoint would have.
    let owner_dir = TempState::new("departure-keyless-owner")?;
    let member_dir = TempState::new("departure-keyless-member")?;
    let owner_config = StoreConfig::new(&owner_dir.0);
    let owner = Runtime::start(owner_config.clone()).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("departure-keyless".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x71; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;
    rusqlite::Connection::open(owner_config.database_path())?
        .execute("UPDATE spaces SET authority_key_ref = NULL", [])?;
    let owner = Runtime::start(owner_config).await?;

    // When: the member asks to leave.
    let refused = timeout(
        PROMPT,
        member
            .handle()
            .leave_space(space, RequestId::try_from([0x72; 16].as_slice())?),
    )
    .await?
    .err()
    .ok_or("a creator without the authority key cannot remove the member")?;

    // Then: the departure is refused, and the creator still answers.
    assert_eq!(
        refused.code(),
        SpaceDepartureErrorCode::REJECTED,
        "{refused}"
    );
    timeout(PROMPT, owner.handle().status()).await??;
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_keyless_creator_refuses_departure_for_an_already_removed_member_without_binding()
-> TestResult {
    // Given: a Space where member M joined, was revoked on the owner before M learned of it,
    // and the owner subsequently lost its authority key.
    let owner_dir = TempState::new("departure-keyless-removed-owner")?;
    let member_dir = TempState::new("departure-keyless-removed-member")?;
    let owner_config = StoreConfig::new(&owner_dir.0);
    let owner = Runtime::start(owner_config.clone()).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("departure-keyless-removed".to_owned())
        .await?;
    let member_id = member.handle().status().await?.endpoint_id();
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x73; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;
    // Commit the owner's revocation directly while owner is stopped so it is never pushed to M,
    // then clear the authority key before restarting the owner.
    {
        let mut repo = ma2a_store::Repository::open(&owner_config)?;
        let chain = repo.load_space_chain(space)?.ok_or("owner chain missing")?;
        let members = chain
            .members()
            .iter()
            .filter(|entry| entry.endpoint_id() != member_id)
            .cloned()
            .collect::<Vec<_>>();
        let mut revocations = chain.revocations().to_vec();
        revocations.push(ma2a_core::SpaceRevocationV1::new(member_id));
        repo.advance_owned_space(&ma2a_store::OwnedSpaceUpdate::new(
            space,
            u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis(),
            )?,
            ma2a_core::SpaceManifestMembership::new(members, revocations),
        ))?;
    }
    rusqlite::Connection::open(owner_config.database_path())?
        .execute("UPDATE spaces SET authority_key_ref = NULL", [])?;
    let owner = Runtime::start(owner_config.clone()).await?;

    // When: M sends a departure request with a fresh RequestId to the keyless creator.
    let refused = timeout(
        PROMPT,
        member
            .handle()
            .leave_space(space, RequestId::try_from([0x74; 16].as_slice())?),
    )
    .await?
    .err()
    .ok_or("a keyless creator must refuse departure for an already-removed member")?;

    // Then: the departure is rejected and no row was written to departure_receipts.
    assert_eq!(
        refused.code(),
        SpaceDepartureErrorCode::REJECTED,
        "{refused}"
    );
    let receipt_count: i64 = rusqlite::Connection::open(owner_config.database_path())?.query_row(
        "SELECT COUNT(*) FROM departure_receipts",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(receipt_count, 0, "keyless creator must not bind a receipt");
    timeout(PROMPT, owner.handle().status()).await??;
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}
