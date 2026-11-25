//! Manifest advances and candidate enrollments invalidate stale synchronized-peer status.
use std::time::Duration;

use ma2a_core::{EndpointId, InviteEntropy, RequestId};
use tokio::time::timeout;

use super::*;
use crate::{EnrollmentAttempt, EnrollmentCreation, state::RuntimeEvent};

const PROMPT: Duration = Duration::from_secs(10);

async fn ipc_sync_peers(client: &LocalApiClient, peer: EndpointId) -> TestResult<Vec<String>> {
    let peer_hex = api::encode_hex(peer.as_bytes());
    let command = api::decode_command(
        format!(
            r#"{{"version":1,"operation":"control_sync_status","peer_endpoint_id":"{peer_hex}"}}"#
        )
        .as_bytes(),
    )?;
    let response: serde_json::Value = serde_json::from_slice(&client.call(&command).await?)?;
    let peers = response
        .pointer("/result/payload/peer_endpoint_ids")
        .and_then(serde_json::Value::as_array)
        .ok_or("peer_endpoint_ids missing")?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "non-string peer id".into())
        })
        .collect::<TestResult<Vec<_>>>()?;
    Ok(peers)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoking_a_peer_through_control_sync_removes_it_from_control_sync_status() -> TestResult {
    // Given: owner A and members B and C in one Space, with B served over IPC and synced with C.
    let state_a = TempState::new_named("sync-peers-a")?;
    let state_b = TempState::new_named("sync-peers-b")?;
    let state_c = TempState::new_named("sync-peers-c")?;
    let owner_a = Runtime::start(StoreConfig::new(&state_a.0)).await?;
    let member_b = Runtime::start(StoreConfig::new(&state_b.0)).await?;
    let member_c = Runtime::start(StoreConfig::new(&state_c.0)).await?;
    let space = owner_a
        .handle()
        .create_owned_space("sync-peers".to_owned())
        .await?;
    for (member, req) in [(&member_b, 0xa1_u8), (&member_c, 0xa2_u8)] {
        let ticket = owner_a
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
                RequestId::try_from([req; 16].as_slice())?,
                format!("member-{req:x}"),
            ))
            .await?;
    }
    // Propagate C's membership and address record to B through A, then sync B with C.
    timeout(PROMPT, member_c.handle().sync_control()).await??;
    timeout(PROMPT, member_b.handle().sync_control()).await??;
    timeout(PROMPT, member_b.handle().sync_control()).await??;

    let paths_b = IpcPaths::new(&state_b.0)?;
    let control_b = CurrentUserRuntime::open_at(
        &state_b.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server_b = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths_b.clone(), member_b.handle(), control_b, None)
            .await?,
    );
    let client_b = LocalApiClient::new(paths_b);
    let id_a = owner_a.handle().status().await?.endpoint_id();
    let id_c = member_c.handle().status().await?.endpoint_id();
    assert_eq!(
        ipc_sync_peers(&client_b, id_c).await?,
        vec![api::encode_hex(id_c.as_bytes())]
    );

    // When: A revokes C and B learns the new manifest through control sync.
    let mut events_b = member_b.handle().subscribe();
    owner_a
        .handle()
        .revoke_owned_space_member(space, id_c)
        .await?;
    timeout(PROMPT, member_b.handle().sync_control()).await??;

    // Then: B immediately stops reporting C as synchronized over IPC, emits memberships_changed,
    // and keeps A synchronized.
    assert_eq!(ipc_sync_peers(&client_b, id_c).await?, Vec::<String>::new());
    assert_eq!(
        ipc_sync_peers(&client_b, id_a).await?,
        vec![api::encode_hex(id_a.as_bytes())]
    );
    let mut announced = false;
    while let Ok(event) = events_b.try_recv() {
        announced |= event == RuntimeEvent::memberships_changed(event.revision());
    }
    assert!(
        announced,
        "B did not emit memberships_changed on manifest advance"
    );
    server_b.cancel().await?;
    timeout(PROMPT, owner_a.shutdown()).await??;
    timeout(PROMPT, member_b.shutdown()).await??;
    timeout(PROMPT, member_c.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enrolling_into_a_second_space_clears_prior_synchronized_peers() -> TestResult {
    // Given: candidate A synchronized with owner P1 in Space S1, and a second Space S2 owned by P2.
    let state_p1 = TempState::new_named("sync-enroll-p1")?;
    let state_p2 = TempState::new_named("sync-enroll-p2")?;
    let state_a = TempState::new_named("sync-enroll-a")?;
    let owner_p1 = Runtime::start(StoreConfig::new(&state_p1.0)).await?;
    let owner_p2 = Runtime::start(StoreConfig::new(&state_p2.0)).await?;
    let member_a = Runtime::start(StoreConfig::new(&state_a.0)).await?;
    let space_1 = owner_p1
        .handle()
        .create_owned_space("space-1".to_owned())
        .await?;
    let space_2 = owner_p2
        .handle()
        .create_owned_space("space-2".to_owned())
        .await?;
    let ticket_1 = owner_p1
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_1,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member_a
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket_1,
            RequestId::try_from([0xa3; 16].as_slice())?,
            "member-a".to_owned(),
        ))
        .await?;
    timeout(PROMPT, member_a.handle().sync_control()).await??;
    let id_p1 = owner_p1.handle().status().await?.endpoint_id();
    assert!(
        member_a
            .handle()
            .control_sync_status(id_p1)
            .await?
            .into_value()
    );

    // When: A enrolls into S2 (owned by P2, so no control sync runs toward P1).
    let ticket_2 = owner_p2
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space_2,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member_a
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket_2,
            RequestId::try_from([0xa4; 16].as_slice())?,
            "member-a".to_owned(),
        ))
        .await?;

    // Then: prior synchronized peers are invalidated on enrollment.
    assert!(
        !member_a
            .handle()
            .control_sync_status(id_p1)
            .await?
            .into_value(),
        "candidate enrollment into a new Space left prior synchronized peers cached"
    );
    timeout(PROMPT, owner_p1.shutdown()).await??;
    timeout(PROMPT, owner_p2.shutdown()).await??;
    timeout(PROMPT, member_a.shutdown()).await??;
    Ok(())
}
