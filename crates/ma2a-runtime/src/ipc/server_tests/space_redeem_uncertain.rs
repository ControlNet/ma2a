//! Uncertain remote enrollment outcomes keep their `RequestId` fenced for exact retry.
use std::net::Ipv4Addr;

use iroh::{
    Endpoint, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use ma2a_core::{EndpointId, RequestId, SpaceId};
use ma2a_store::{AuthorizedEnrollmentRedemption, KeyKind, KeyReference, KeyStore, Repository};
use tokio::time::timeout;

use super::{
    space_redeem_support::{
        PROMPT, ServedRuntime, arm_address_fault, call_json, create_space_and_invite,
        invitation_row, serve_runtime,
    },
    *,
};

#[derive(Clone, Debug)]
struct CommitThenDropReply {
    owner_config: StoreConfig,
    owner_id: EndpointId,
}

impl ProtocolHandler for CommitThenDropReply {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote: EndpointId = connection.remote_id().into();
        let (_send, mut receive) = connection.accept_bi().await?;
        let request = receive
            .read_to_end(4_096)
            .await
            .map_err(std::io::Error::other)?;
        let (ticket, redemption) = crate::enrollment::decode_attempt(&request, remote)
            .map_err(|_| std::io::Error::other("decode attempt failed"))?;
        let now_ms = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(std::io::Error::other)?
                .as_millis(),
        )
        .map_err(std::io::Error::other)?;
        let authorized = AuthorizedEnrollmentRedemption::new(ticket, redemption, now_ms)
            .requiring_owner_address(self.owner_id);
        let mut repo = Repository::open(&self.owner_config).map_err(std::io::Error::other)?;
        let _outcome = repo
            .redeem_enrollment(&authorized)
            .map_err(std::io::Error::other)?;
        // Drop the reply after the owner has durably committed generation N+1.
        connection.close(1_u8.into(), b"lost reply");
        Ok(())
    }
}

async fn start_dropping_owner(
    owner_state: &TempState,
    owner_config: StoreConfig,
    (owner_id, bind_port): (EndpointId, u16),
) -> TestResult<Router> {
    let protected = KeyStore::open(&owner_state.0)?.read(
        KeyKind::Endpoint,
        &KeyReference::parse("endpoint-identity-v1")?,
    )?;
    let secret = SecretKey::from_bytes(&<[u8; 32]>::try_from(protected.as_ref())?);
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind_addr((Ipv4Addr::UNSPECIFIED, bind_port))?
        .alpns(vec![ma2a_net::ENROLLMENT_ALPN.to_vec()])
        .bind()
        .await?;
    Ok(Router::builder(endpoint)
        .accept(
            ma2a_net::ENROLLMENT_ALPN,
            CommitThenDropReply {
                owner_config,
                owner_id,
            },
        )
        .spawn())
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

async fn assert_transient_refusal_then_recovery(
    (owner, owner_config): (&Runtime, &StoreConfig),
    (cand_client, cand_config): (&LocalApiClient, &StoreConfig),
    (space, redeem_body, changed_body): (SpaceId, &serde_json::Value, &serde_json::Value),
) -> TestResult {
    // A subsequent no-effect retry refusal (missing owner address) still keeps the fence.
    arm_address_fault(owner, 4);
    rusqlite::Connection::open(owner_config.database_path())?.execute(
        "DELETE FROM address_state WHERE space_id = ?1",
        [space.as_bytes().as_slice()],
    )?;
    let transient = call_json(cand_client, redeem_body.clone()).await?;
    assert_eq!(
        transient.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{transient}"
    );
    assert_conflict(cand_client, changed_body).await?;

    // Once owner recovers its address, exact retry recovers the consumed invitation.
    owner.handle().clear_background_faults();
    owner.handle().publish_address().await?;
    let recovered = call_json(cand_client, redeem_body.clone()).await?;
    assert_eq!(recovered.get("error"), None, "{recovered}");
    assert_eq!(
        recovered
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("space_redeemed")
    );
    let owner_chain = Repository::open(owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    let cand_chain = Repository::open(cand_config)?
        .load_space_chain(space)?
        .ok_or("candidate chain missing")?;
    assert_eq!(owner_chain, cand_chain);
    assert_eq!(owner_chain.latest_generation(), 1);
    assert_eq!(owner_chain.members().len(), 2);

    let replayed = call_json(cand_client, redeem_body.clone()).await?;
    assert_eq!(replayed, recovered);
    assert_conflict(cand_client, changed_body).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_response_after_owner_commit_fences_request_id_and_recovers_on_exact_retry()
-> TestResult {
    let owner = ServedRuntime::start("enroll-ipc-uncertain-owner").await?;
    let owner_id = owner.runtime.handle().status().await?.endpoint_id();
    let (_space_hex, ticket_str, ticket) = create_space_and_invite(
        &owner,
        "ipc-uncertain",
        (
            "f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1f1",
            "f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2f2",
        ),
    )
    .await?;
    let space = ticket.space_id();
    let owner_config = owner.config.clone();
    let bind_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let owner_state = owner.stop_preserving_state().await?;

    let dropping_owner =
        start_dropping_owner(&owner_state, owner_config.clone(), (owner_id, bind_port)).await?;

    let candidate = ServedRuntime::start("enroll-ipc-uncertain-cand").await?;
    let req_1 = "f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3f3";
    let req_1_id = RequestId::try_from([0xf3; 16].as_slice())?;
    let redeem_body = serde_json::json!({
        "version": 1,
        "operation": "space_redeem",
        "request_id": req_1,
        "invitation": ticket_str,
    });
    let changed_body = serde_json::json!({
        "version": 1,
        "operation": "space_create",
        "request_id": req_1,
        "name": "different-work",
    });

    let uncertain = call_json(&candidate.client, redeem_body.clone()).await?;
    assert_eq!(
        uncertain.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::INTERNAL.name()),
        "{uncertain}"
    );
    let remediation = uncertain
        .get("remediation")
        .and_then(serde_json::Value::as_str)
        .ok_or("expected remediation reporting possible remote effects")?;
    assert!(
        remediation.contains("remote") && remediation.contains("retry"),
        "unexpected remediation: {remediation}"
    );
    assert_eq!(
        invitation_row(&owner_config, &ticket.invitation_id())?,
        (1, Some(req_1_id.as_bytes().to_vec()))
    );

    dropping_owner.shutdown().await?;
    drop(dropping_owner);
    let owner = Runtime::start(owner_config.clone()).await?;

    assert_conflict(&candidate.client, &changed_body).await?;
    assert!(
        candidate
            .runtime
            .handle()
            .snapshot()
            .await?
            .spaces()
            .is_empty()
    );

    let cand_config = candidate.config.clone();
    let cand_state = candidate.stop_preserving_state().await?;
    let candidate = Runtime::start(cand_config.clone()).await?;
    let (cand_server, cand_client) = serve_runtime(&cand_state, &candidate).await?;
    assert_conflict(&cand_client, &changed_body).await?;

    assert_transient_refusal_then_recovery(
        (&owner, &owner_config),
        (&cand_client, &cand_config),
        (space, &redeem_body, &changed_body),
    )
    .await?;

    cand_server.cancel().await?;
    timeout(PROMPT, candidate.shutdown()).await??;
    timeout(PROMPT, owner.shutdown()).await??;
    drop(cand_state);
    drop(owner_state);
    Ok(())
}
