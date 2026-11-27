//! Missing owner address refuses first-use enrollment without consuming the ticket.
use ma2a_core::{RequestId, SpaceId};
use ma2a_store::Repository;

use super::{
    space_redeem_support::{
        ServedRuntime, arm_address_fault, call_json, create_space_and_invite, invitation_row,
        invite_over_ipc,
    },
    *,
};

fn assert_converged(
    owner: &ServedRuntime,
    candidate: &ServedRuntime,
    (space, expected_generation, invitation_id): (SpaceId, u64, &[u8; 16]),
) -> TestResult {
    let owner_chain = Repository::open(&owner.config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    let cand_chain = Repository::open(&candidate.config)?
        .load_space_chain(space)?
        .ok_or("candidate chain missing")?;
    assert_eq!(owner_chain, cand_chain);
    assert_eq!(owner_chain.latest_generation(), expected_generation);
    assert_eq!(owner_chain.members().len(), 2);
    assert_eq!(invitation_row(&owner.config, invitation_id)?.0, 1);
    Ok(())
}

async fn assert_same_id_retry_after_missing_address(
    owner: &ServedRuntime,
    space: SpaceId,
    space_hex: &str,
) -> TestResult {
    arm_address_fault(&owner.runtime, 4);
    let (ticket_str, _) = invite_over_ipc(
        owner,
        space_hex,
        ("d5d5d5d5d5d5d5d5d5d5d5d5d5d5d5d5", "invite2.ticket"),
    )
    .await?;
    let second = ServedRuntime::start("enroll-ipc-addr-second").await?;
    rusqlite::Connection::open(owner.config.database_path())?.execute(
        "DELETE FROM address_state WHERE space_id = ?1",
        [space.as_bytes().as_slice()],
    )?;
    let req_3 = "d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6d6";
    let first_try = call_json(
        &second.client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": req_3,
            "invitation": ticket_str,
        }),
    )
    .await?;
    assert_eq!(
        first_try.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{first_try}"
    );
    owner.runtime.handle().clear_background_faults();
    owner.runtime.handle().publish_address().await?;
    let same_id_retry = call_json(
        &second.client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": req_3,
            "invitation": ticket_str,
        }),
    )
    .await?;
    assert_eq!(same_id_retry.get("error"), None, "{same_id_retry}");
    second.shutdown().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_owner_address_refuses_first_use_without_effect_through_both_ipcs() -> TestResult {
    let owner = ServedRuntime::start("enroll-ipc-addr-owner").await?;
    let candidate = ServedRuntime::start("enroll-ipc-addr-cand").await?;

    arm_address_fault(&owner.runtime, 4);
    let (space_hex, ticket_str, ticket) = create_space_and_invite(
        &owner,
        "ipc-missing-addr",
        (
            "d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1",
            "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2",
        ),
    )
    .await?;
    let space = ticket.space_id();
    let owner_id = owner.runtime.handle().status().await?.endpoint_id();
    assert!(
        Repository::open(&owner.config)?
            .address_record(space, owner_id)?
            .is_none()
    );
    let before_revision = owner.runtime.handle().status().await?.revision();
    let before_generation = Repository::open(&owner.config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();

    arm_address_fault(&owner.runtime, 4);
    let refused = call_json(
        &candidate.client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": "d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3",
            "invitation": ticket_str,
        }),
    )
    .await?;
    assert_eq!(
        refused.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{refused}"
    );
    assert_eq!(refused.get("remediation"), Some(&serde_json::Value::Null));
    let req_1_id = RequestId::try_from([0xd3; 16].as_slice())?;
    assert_eq!(
        candidate.runtime.handle().mutation_replay(req_1_id).await?,
        None
    );
    assert!(owner.runtime.handle().status().await?.is_ready());
    assert_eq!(
        invitation_row(&owner.config, &ticket.invitation_id())?,
        (0, None)
    );
    assert_eq!(
        owner.runtime.handle().status().await?.revision(),
        before_revision
    );

    owner.runtime.handle().clear_background_faults();
    owner.runtime.handle().publish_address().await?;
    let redeemed = call_json(
        &candidate.client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4",
            "invitation": ticket_str,
        }),
    )
    .await?;
    assert_eq!(redeemed.get("error"), None, "{redeemed}");
    assert_eq!(
        redeemed
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("space_redeemed")
    );
    assert_converged(
        &owner,
        &candidate,
        (space, before_generation + 1, &ticket.invitation_id()),
    )?;

    assert_same_id_retry_after_missing_address(&owner, space, &space_hex).await?;

    candidate.shutdown().await?;
    owner.shutdown().await
}
