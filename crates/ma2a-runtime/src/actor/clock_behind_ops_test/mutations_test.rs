use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use iroh_tickets::Ticket as _;
use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::{MutableClock, NOW_MS, PROMPT, TempState, TestResult};
use crate::{
    EnrollmentAttempt, EnrollmentCreation, Runtime,
    actor::maintenance::FaultPoint,
    api,
    current_user::CurrentUserRuntime,
    error::{RuntimeError, RuntimeErrorKind},
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{SystemClock, WebAuthConfig},
};

pub(super) async fn spawn_ipc(
    dir: &std::path::Path,
    handle: crate::RuntimeHandle,
) -> Result<
    (
        LocalApiClient,
        CancellationToken,
        tokio::task::JoinHandle<Result<crate::ipc::ServerExit, crate::ipc::IpcError>>,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let paths = IpcPaths::new(dir)?;
    let control = CurrentUserRuntime::open_at(
        dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let stop = CancellationToken::new();
    let server = LocalApiServer::bind_for_launch(paths.clone(), handle, control, None).await?;
    let task = tokio::spawn(server.serve(stop.child_token()));
    Ok((LocalApiClient::new(paths), stop, task))
}

pub(super) async fn expect_ipc_ok(
    (client, handle): (&LocalApiClient, &crate::RuntimeHandle),
    payload: serde_json::Value,
    expected_type: Option<&str>,
) -> TestResult {
    let command = api::decode_command(serde_json::to_vec(&payload)?.as_slice())?;
    let request_id = command.request_id().ok_or("request_id missing")?;
    let reply = timeout(PROMPT, client.call(&command)).await??;
    let json: serde_json::Value = serde_json::from_slice(&reply)?;
    assert_eq!(json.get("error"), None, "ipc call failed: {json}");
    if let Some(expected) = expected_type {
        assert_eq!(
            json.pointer("/result/type")
                .and_then(serde_json::Value::as_str),
            Some(expected)
        );
    }
    let recorded = handle.mutation_replay(request_id).await?;
    assert!(
        recorded.is_some(),
        "missing replay record for {request_id:?}"
    );
    Ok(())
}

pub(super) async fn start_clock(
    label: &str,
) -> Result<(TempState, Arc<MutableClock>, Runtime), Box<dyn std::error::Error + Send + Sync>> {
    let dir = TempState::new(label)?;
    let clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(StoreConfig::new(&dir.0), Arc::<MutableClock>::clone(&clock))
            .await?;
    Ok((dir, clock, runtime))
}

async fn invite(
    owner: &Runtime,
    space: ma2a_core::SpaceId,
) -> Result<ma2a_core::SignedInviteTicket, Box<dyn std::error::Error + Send + Sync>> {
    Ok(owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_space_mutations_succeed_over_ipc_under_clock_behind() -> TestResult {
    // Given: owner and member runtimes, each with an unrelated Space ahead of the restored clock.
    let (owner_dir, owner_clock, owner) = start_clock("a05-owner").await?;
    let (member_dir, member_clock, member) = start_clock("a05-member").await?;
    let (_revokee_dir, _revokee_clock, revokee) = start_clock("a05-revokee").await?;

    let space = owner
        .handle()
        .create_owned_space("shared".to_owned())
        .await?;
    let revoke_ticket = invite(&owner, space).await?;
    revokee
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            revoke_ticket,
            RequestId::try_from([0xb0; 16].as_slice())?,
            "revokee".to_owned(),
        ))
        .await?;
    let revokee_id = revokee.handle().status().await?.endpoint_id();

    for (runtime, clock, name) in [
        (&owner, &owner_clock, "owner-ahead"),
        (&member, &member_clock, "member-ahead"),
    ] {
        clock.0.store(NOW_MS + 30_000, Ordering::Relaxed);
        let _ahead = runtime.handle().create_owned_space(name.to_owned()).await?;
        clock.0.store(NOW_MS, Ordering::Relaxed);
    }

    let (owner_client, owner_stop, owner_server_task) =
        spawn_ipc(&owner_dir.0, owner.handle()).await?;
    let (member_client, member_stop, member_server_task) =
        spawn_ipc(&member_dir.0, member.handle()).await?;

    // 1. space_create under ClockBehind
    expect_ipc_ok(
        (&owner_client, &owner.handle()),
        serde_json::json!({
            "version": 1, "operation": "space_create",
            "request_id": "b1".repeat(16), "name": "under-rollback",
        }),
        Some("space_created"),
    )
    .await?;

    // 2. space_redeem under ClockBehind
    let member_ticket = invite(&owner, space).await?;
    expect_ipc_ok(
        (&member_client, &member.handle()),
        serde_json::json!({
            "version": 1, "operation": "space_redeem",
            "request_id": "b2".repeat(16), "invitation": member_ticket.encode_string(),
        }),
        Some("space_redeemed"),
    )
    .await?;

    // 3. space_grant under ClockBehind
    let member_id = member.handle().status().await?.endpoint_id();
    expect_ipc_ok(
        (&owner_client, &owner.handle()),
        serde_json::json!({
            "version": 1, "operation": "space_grant",
            "request_id": "b3".repeat(16),
            "space_id": api::encode_hex(space.as_bytes()),
            "peer_endpoint_id": api::encode_hex(member_id.as_bytes()),
            "files_read": true, "files_write": false, "exec": true, "shell": false,
            "proxy": false, "process": false, "agent": false, "service": false,
        }),
        None,
    )
    .await?;

    // 4. space_leave under ClockBehind
    expect_ipc_ok(
        (&member_client, &member.handle()),
        serde_json::json!({
            "version": 1, "operation": "space_leave",
            "request_id": "b4".repeat(16),
            "space_id": api::encode_hex(space.as_bytes()),
        }),
        Some("space_left"),
    )
    .await?;

    // 5. space_revoke under ClockBehind
    expect_ipc_ok(
        (&owner_client, &owner.handle()),
        serde_json::json!({
            "version": 1, "operation": "space_revoke",
            "request_id": "b5".repeat(16),
            "space_id": api::encode_hex(space.as_bytes()),
            "peer_endpoint_id": api::encode_hex(revokee_id.as_bytes()),
        }),
        Some("space_revoked"),
    )
    .await?;

    owner_stop.cancel();
    member_stop.cancel();
    let _ = owner_server_task.await;
    let _ = member_server_task.await;
    timeout(PROMPT, revokee.shutdown()).await??;
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_create_and_redeem_succeed_under_address_observation_unavailable() -> TestResult {
    let owner_dir = TempState::new("a05-obs-owner")?;
    let candidate_dir = TempState::new("a05-obs-cand")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let candidate = Runtime::start(StoreConfig::new(&candidate_dir.0)).await?;

    for _ in 0..2 {
        owner.handle().fail_background_once(
            FaultPoint::AddressPublication,
            RuntimeError::new(RuntimeErrorKind::AddressObservationUnavailable),
        );
    }
    let space = owner
        .handle()
        .create_owned_space("obs-unavailable".to_owned())
        .await?;
    let ticket = invite(&owner, space).await?;

    for _ in 0..2 {
        candidate.handle().fail_background_once(
            FaultPoint::AddressPublication,
            RuntimeError::new(RuntimeErrorKind::AddressObservationUnavailable),
        );
    }
    timeout(
        PROMPT,
        candidate.handle().redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0xb6; 16].as_slice())?,
            "candidate".to_owned(),
        )),
    )
    .await??;
    assert!(
        candidate
            .handle()
            .status()
            .await?
            .memberships
            .contains(&space)
    );

    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, candidate.shutdown()).await??;
    Ok(())
}
