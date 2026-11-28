//! Post-commit bootstrap failures must keep the candidate's `RequestId` fenced across restart.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI64, Ordering},
};

use ma2a_core::RequestId;
use ma2a_store::Repository;
use tokio::time::timeout;

use super::{
    space_redeem_support::{PROMPT, arm_address_fault, call_json, invitation_row, serve_runtime},
    *,
};
use crate::RuntimeClock;

const NOW_MS: i64 = 1_700_000_000_000;

#[derive(Debug)]
struct PostCommitBackstepClock {
    now_ms: AtomicI64,
    armed: AtomicBool,
    backstep_to_ms: AtomicI64,
}

impl PostCommitBackstepClock {
    const fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
            armed: AtomicBool::new(false),
            backstep_to_ms: AtomicI64::new(now_ms),
        }
    }

    fn set(&self, now_ms: i64) {
        self.armed.store(false, Ordering::SeqCst);
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }

    fn step_back_after_first_read(&self, first_ms: i64, after_ms: i64) {
        self.now_ms.store(first_ms, Ordering::SeqCst);
        self.backstep_to_ms.store(after_ms, Ordering::SeqCst);
        self.armed.store(true, Ordering::SeqCst);
    }
}

impl RuntimeClock for PostCommitBackstepClock {
    fn now_ms(&self) -> Result<i64, crate::RuntimeError> {
        let current = self.now_ms.load(Ordering::SeqCst);
        if self.armed.swap(false, Ordering::SeqCst) {
            let after = self.backstep_to_ms.load(Ordering::SeqCst);
            self.now_ms.store(after, Ordering::SeqCst);
        }
        Ok(current)
    }
}

async fn assert_conflict(client: &LocalApiClient, body: &serde_json::Value) -> TestResult {
    let reply = call_json(client, body.clone()).await?;
    assert_eq!(
        reply.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{reply}"
    );
    Ok(())
}

async fn create_invite_at_clock(
    owner_state: &TempState,
    owner_client: &LocalApiClient,
) -> TestResult<(String, ma2a_core::SignedInviteTicket)> {
    let created = call_json(
        owner_client,
        serde_json::json!({
            "version": 1, "operation": "space_create",
            "request_id": "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1", "name": "post-commit",
        }),
    )
    .await?;
    let space_hex = created
        .pointer("/result/payload/space_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("space_id missing")?
        .to_owned();
    let ticket_path = owner_state.0.join("invite.ticket");
    let invited = call_json(
        owner_client,
        serde_json::json!({
            "version": 1, "operation": "space_invite",
            "request_id": "a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2",
            "space_id": space_hex, "ttl_ms": 300_000,
            "output_path": ticket_path.to_str().ok_or("invalid ticket path")?,
        }),
    )
    .await?;
    assert_eq!(invited.get("error"), None, "{invited}");
    let ticket_str = std::fs::read_to_string(&ticket_path)?.trim().to_owned();
    let ticket =
        <ma2a_core::SignedInviteTicket as iroh_tickets::Ticket>::decode_string(&ticket_str)?;
    Ok((ticket_str, ticket))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_commit_bootstrap_clock_backstep_keeps_fence_across_restart_and_recovers() -> TestResult
{
    let owner_state = TempState::new_named("enroll-post-commit-owner")?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let owner_clock = Arc::new(PostCommitBackstepClock::new(NOW_MS));
    let owner_runtime = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::<PostCommitBackstepClock>::clone(&owner_clock),
    )
    .await?;
    let (owner_server, owner_client) = serve_runtime(&owner_state, &owner_runtime).await?;

    let cand_state = TempState::new_named("enroll-post-commit-cand")?;
    let cand_config = StoreConfig::new(&cand_state.0);
    let cand_clock = Arc::new(PostCommitBackstepClock::new(NOW_MS + 10_000));
    let cand_runtime = Runtime::start_with_clock(
        cand_config.clone(),
        Arc::<PostCommitBackstepClock>::clone(&cand_clock),
    )
    .await?;
    let (cand_server, cand_client) = serve_runtime(&cand_state, &cand_runtime).await?;

    let (ticket_str, ticket) = create_invite_at_clock(&owner_state, &owner_client).await?;
    let space = ticket.space_id();

    // Let redemption commit at NOW_MS + 1_000, then step the owner clock behind the persisted
    // address record's issue time so post-commit bootstrap validation fails.
    arm_address_fault(&owner_runtime, 4);
    owner_clock.step_back_after_first_read(NOW_MS + 1_000, NOW_MS - 10_000);

    let req_1 = "a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3";
    let req_1_id = RequestId::try_from([0xa3; 16].as_slice())?;
    let redeem_body = serde_json::json!({
        "version": 1, "operation": "space_redeem", "request_id": req_1, "invitation": ticket_str,
    });
    let changed_body = serde_json::json!({
        "version": 1, "operation": "space_create", "request_id": req_1, "name": "other-work",
    });

    let first = call_json(&cand_client, redeem_body.clone()).await?;
    assert_eq!(
        first.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{first}"
    );
    let remediation = first
        .get("remediation")
        .and_then(serde_json::Value::as_str)
        .ok_or("post-commit bootstrap failure must carry remote-effect remediation")?;
    assert!(
        remediation.contains("remote") && remediation.contains("retry"),
        "unexpected remediation: {remediation}"
    );
    assert_eq!(
        invitation_row(&owner_config, &ticket.invitation_id())?,
        (1, Some(req_1_id.as_bytes().to_vec()))
    );
    assert!(owner_runtime.handle().status().await?.is_ready());
    assert_conflict(&cand_client, &changed_body).await?;

    // Restart candidate and verify the fence survives and exact retry completes once clock recovers.
    cand_server.cancel().await?;
    IpcPaths::new(&cand_state.0)?.remove_stale_endpoint()?;
    timeout(PROMPT, cand_runtime.shutdown()).await??;
    let cand_runtime = Runtime::start_with_clock(
        cand_config.clone(),
        Arc::<PostCommitBackstepClock>::clone(&cand_clock),
    )
    .await?;
    let (cand_server, cand_client) = serve_runtime(&cand_state, &cand_runtime).await?;
    assert_conflict(&cand_client, &changed_body).await?;

    owner_clock.set(NOW_MS + 2_000);
    owner_runtime.handle().clear_background_faults();
    owner_runtime.handle().publish_address().await?;
    let recovered = call_json(&cand_client, redeem_body).await?;
    assert_eq!(recovered.get("error"), None, "{recovered}");
    assert_eq!(
        recovered
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("space_redeemed")
    );
    let owner_chain = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    let cand_chain = Repository::open(&cand_config)?
        .load_space_chain(space)?
        .ok_or("candidate chain missing")?;
    assert_eq!(owner_chain, cand_chain);
    assert_eq!(owner_chain.latest_generation(), 1);

    cand_server.cancel().await?;
    owner_server.cancel().await?;
    timeout(PROMPT, cand_runtime.shutdown()).await??;
    timeout(PROMPT, owner_runtime.shutdown()).await??;
    Ok(())
}
