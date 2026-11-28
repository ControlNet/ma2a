//! An already-consumed invitation remains recoverable with its exact `RequestId` over candidate IPC.
use ma2a_core::{RequestId, SignedInviteTicket};
use ma2a_store::{KeyKind, KeyReference, KeyStore, Repository};
use tokio::time::timeout;

use super::{
    space_redeem_support::{
        PROMPT, ServedRuntime, arm_address_fault, call_json, create_space_and_invite,
        invitation_row, serve_runtime,
    },
    *,
};
use crate::EnrollmentAttempt;

async fn consume_without_persisting(
    cand_state: &TempState,
    ticket: &SignedInviteTicket,
    request_id: RequestId,
) -> TestResult {
    let protected = KeyStore::open(&cand_state.0)?.read(
        KeyKind::Endpoint,
        &KeyReference::parse("endpoint-identity-v1")?,
    )?;
    let cand_secret = ma2a_net::EndpointSecret::parse(protected.as_ref())?;
    let (tx, _rx) = tokio::sync::mpsc::channel(1);
    let raw_cand = ma2a_net::RuntimeEndpoint::bind(cand_secret, tx, None).await?;
    let raw_request = crate::enrollment::encode_attempt(&EnrollmentAttempt::new(
        ticket.clone(),
        request_id,
        "candidate".to_owned(),
    ))?;
    let (status, _pages) = timeout(
        PROMPT,
        raw_cand.exchange_redemption(ticket.owner_addr(), raw_request),
    )
    .await??;
    assert_eq!(status, 0);
    raw_cand.shutdown().await?;
    Ok(())
}

async fn assert_owner_replay_bootstrap_fenced(
    cand_client: &LocalApiClient,
    first_attempt: &serde_json::Value,
    req_1: &str,
) -> TestResult {
    assert_eq!(
        first_attempt
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{first_attempt}"
    );
    let remediation = first_attempt
        .get("remediation")
        .and_then(serde_json::Value::as_str)
        .ok_or("owner-replay bootstrap failure must carry remote-effect remediation")?;
    assert!(
        remediation.contains("remote") && remediation.contains("retry"),
        "unexpected remediation: {remediation}"
    );
    let reused = call_json(
        cand_client,
        serde_json::json!({
            "version": 1,
            "operation": "space_create",
            "request_id": req_1,
            "name": "conflict-check",
        }),
    )
    .await?;
    assert_eq!(
        reused.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{reused}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consumed_invitation_recovers_with_exact_request_id_through_candidate_ipc() -> TestResult {
    let owner = ServedRuntime::start("enroll-ipc-retry-owner").await?;
    let cand_state = TempState::new_named("enroll-ipc-retry-cand")?;
    let cand_config = StoreConfig::new(&cand_state.0);
    let candidate = Runtime::start(cand_config.clone()).await?;

    let (_space_hex, ticket_str, ticket) = create_space_and_invite(
        &owner,
        "ipc-retry-consumed",
        (
            "e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1",
            "e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2",
        ),
    )
    .await?;
    let space = ticket.space_id();

    let req_1 = "e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3e3";
    let req_1_id = RequestId::try_from([0xe3; 16].as_slice())?;
    timeout(PROMPT, candidate.shutdown()).await??;
    consume_without_persisting(&cand_state, &ticket, req_1_id).await?;
    assert_eq!(
        invitation_row(&owner.config, &ticket.invitation_id())?,
        (1, Some(req_1_id.as_bytes().to_vec()))
    );

    let candidate = Runtime::start(cand_config.clone()).await?;
    let (cand_server, cand_client) = serve_runtime(&cand_state, &candidate).await?;
    arm_address_fault(&owner.runtime, 4);
    rusqlite::Connection::open(owner.config.database_path())?.execute(
        "DELETE FROM address_state WHERE space_id = ?1",
        [space.as_bytes().as_slice()],
    )?;

    let first_attempt = call_json(
        &cand_client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": req_1,
            "invitation": ticket_str,
        }),
    )
    .await?;
    assert_owner_replay_bootstrap_fenced(&cand_client, &first_attempt, req_1).await?;
    assert!(owner.runtime.handle().status().await?.is_ready());

    owner.runtime.handle().clear_background_faults();
    owner.runtime.handle().publish_address().await?;
    let fresh = call_json(
        &cand_client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": "e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4e4",
            "invitation": ticket_str,
        }),
    )
    .await?;
    assert_eq!(
        fresh.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{fresh}"
    );

    let recovered = call_json(
        &cand_client,
        serde_json::json!({
            "version": 1,
            "operation": "space_redeem",
            "request_id": req_1,
            "invitation": ticket_str,
        }),
    )
    .await?;

    assert_eq!(recovered.get("error"), None, "{recovered}");
    assert_eq!(
        recovered
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("space_redeemed")
    );
    let owner_chain = Repository::open(&owner.config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    let cand_chain = Repository::open(&cand_config)?
        .load_space_chain(space)?
        .ok_or("candidate chain missing")?;
    assert_eq!(owner_chain, cand_chain);
    assert_eq!(owner_chain.latest_generation(), 1);
    assert_eq!(owner_chain.members().len(), 2);

    cand_server.cancel().await?;
    timeout(PROMPT, candidate.shutdown()).await??;
    owner.shutdown().await
}
