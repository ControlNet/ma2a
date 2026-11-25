//! Redemption into a full Space refuses without effect so the same request ID can retry later.
use std::time::Duration;

use iroh_tickets::Ticket as _;
use ma2a_core::{
    EndpointId, InviteEntropy, MemberCapabilities, SpaceAuthoritySecret, SpaceId,
    SpaceManifestMembership, SpaceMemberV1,
};
use ma2a_store::{OwnedSpaceUpdate, Repository, StoreError};
use tokio::time::timeout;

use super::*;
use crate::{EnrollmentCreation, actor::maintenance::FaultPoint, error::RuntimeError};

const PROMPT: Duration = Duration::from_secs(10);

fn synthetic_member(seed: u8) -> TestResult<SpaceMemberV1> {
    let key = SpaceAuthoritySecret::from_bytes([seed; 32]).public_key();
    Ok(SpaceMemberV1::new(
        EndpointId::try_from(key.as_bytes().as_slice())?,
        format!("filler-{seed}"),
        MemberCapabilities::new(true, false),
    )?)
}

fn redeem_command(request_id: &str, ticket: &str) -> TestResult<api::Command> {
    Ok(api::decode_command(
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": request_id,
            "invitation": ticket,
        }))?
        .as_slice(),
    )?)
}

fn invitation_status(owner_config: &StoreConfig, invitation_id: &[u8; 16]) -> TestResult<u8> {
    Ok(
        rusqlite::Connection::open(owner_config.database_path())?.query_row(
            "SELECT status FROM invitations WHERE invitation_id = ?1",
            [invitation_id.as_slice()],
            |row| row.get(0),
        )?,
    )
}

async fn fill_space_to_capacity(
    owner: &Runtime,
    owner_config: &StoreConfig,
    space: SpaceId,
) -> TestResult<EndpointId> {
    let initial_chain = Repository::open(owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    let mut members = initial_chain.members().to_vec();
    let removable = synthetic_member(1)?.endpoint_id();
    for seed in 1..=63 {
        members.push(synthetic_member(seed)?);
    }
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    assert_eq!(members.len(), 64);
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    owner
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            space,
            now_ms,
            SpaceManifestMembership::new(members, Vec::new()),
        ))
        .await?;
    Ok(removable)
}

async fn assert_post_commit_failure_stays_fenced(
    owner: &Runtime,
    candidate: &Runtime,
    client: &LocalApiClient,
) -> TestResult {
    let space = owner
        .handle()
        .create_owned_space("fenced-space".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    candidate.handle().fail_background_once(
        FaultPoint::EnrollmentCompletion,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );
    let command = redeem_command("c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2", &ticket.encode_string())?;
    let request_id = command.request_id().ok_or("request id missing")?;
    let failed: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&command)).await??)?;
    assert_eq!(
        failed.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::INTERNAL.name()),
        "{failed}"
    );
    assert!(
        candidate
            .handle()
            .mutation_replay(request_id)
            .await?
            .is_some()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redemption_into_a_full_space_retries_with_the_same_request_id() -> TestResult {
    // Given: an owned Space filled to its 64-member limit and an unused invitation.
    let owner_state = TempState::new_named("redeem-full-owner")?;
    let candidate_state = TempState::new_named("redeem-full-candidate")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let owner = Runtime::start(owner_config.clone()).await?;
    let candidate = Runtime::start(StoreConfig::new(&candidate_state.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("full-space".to_owned())
        .await?;
    let removable = fill_space_to_capacity(&owner, &owner_config, space).await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    let before_generation = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();
    let paths = IpcPaths::new(&candidate_state.0)?;
    let control = CurrentUserRuntime::open_at(
        &candidate_state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), candidate.handle(), control, None).await?,
    );
    let client = LocalApiClient::new(paths);

    // When (1): the candidate redeems while all 64 member slots are occupied.
    let command = redeem_command("c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1", &ticket.encode_string())?;
    let request_id = command.request_id().ok_or("request id missing")?;
    let refused: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&command)).await??)?;

    // Then (1): refused as conflict without effect, releasing the request ID and keeping ticket.
    assert_eq!(
        refused.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{refused}"
    );
    assert_eq!(refused.get("remediation"), Some(&serde_json::Value::Null));
    assert_eq!(candidate.handle().mutation_replay(request_id).await?, None);
    let gen_after_refusal = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();
    assert_eq!(gen_after_refusal, before_generation);
    assert_eq!(
        invitation_status(&owner_config, &ticket.invitation_id())?,
        0
    );

    // When (2): a slot opens and the candidate retries with the exact same request ID.
    owner
        .handle()
        .revoke_owned_space_member(space, removable)
        .await?;
    let gen_after_revoke = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();
    let redeemed: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&command)).await??)?;

    // Then (2): redemption succeeds, consuming the ticket and advancing generation once.
    assert_eq!(redeemed.get("error"), None, "{redeemed}");
    assert_eq!(
        redeemed
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("space_redeemed")
    );
    let gen_after_redeem = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();
    assert_eq!(gen_after_redeem, gen_after_revoke + 1);
    assert_eq!(
        invitation_status(&owner_config, &ticket.invitation_id())?,
        1
    );
    assert!(
        candidate
            .handle()
            .status()
            .await?
            .memberships
            .contains(&space)
    );

    // And (3): a post-commit failure remains fenced under its request ID.
    assert_post_commit_failure_stays_fenced(&owner, &candidate, &client).await?;

    server.cancel().await?;
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}
