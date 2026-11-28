//! In-flight initial `space_redeem` stays exclusive and recovers as exact-retryable after crash.
use std::{net::Ipv4Addr, sync::Arc};

use iroh::{
    Endpoint, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use ma2a_core::{EndpointId, RequestId};
use ma2a_store::{AuthorizedEnrollmentRedemption, KeyKind, KeyReference, KeyStore, Repository};
use tokio::{sync::Notify, time::timeout};
use tokio_util::sync::CancellationToken;

use super::{
    space_redeem_support::{
        PROMPT, ServedRuntime, arm_address_fault, call_json, create_space_and_invite,
        invitation_row, serve_runtime,
    },
    *,
};

#[derive(Clone, Debug)]
struct CommitAndHoldReply {
    owner_config: StoreConfig,
    owner_id: EndpointId,
    committed: Arc<Notify>,
    release: CancellationToken,
}

impl ProtocolHandler for CommitAndHoldReply {
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
        self.committed.notify_waiters();
        self.release.cancelled().await;
        Ok(())
    }
}

async fn start_holding_owner(
    owner_state: &TempState,
    owner_config: StoreConfig,
    (owner_id, bind_port): (EndpointId, u16),
) -> TestResult<(Router, Arc<Notify>, CancellationToken)> {
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
    let committed = Arc::new(Notify::new());
    let release = CancellationToken::new();
    let router = Router::builder(endpoint)
        .accept(
            ma2a_net::ENROLLMENT_ALPN,
            CommitAndHoldReply {
                owner_config,
                owner_id,
                committed: Arc::clone(&committed),
                release: release.clone(),
            },
        )
        .spawn();
    Ok((router, committed, release))
}

async fn assert_in_flight_exclusive_and_retryable(
    candidate: &ServedRuntime,
    (owner_config, invitation_id): (&StoreConfig, &[u8; 16]),
    (req_1_id, redeem_body, changed_body): (RequestId, &serde_json::Value, &serde_json::Value),
) -> TestResult {
    let dup = call_json(&candidate.client, redeem_body.clone()).await?;
    assert_eq!(
        dup.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{dup}"
    );
    let conflict = call_json(&candidate.client, changed_body.clone()).await?;
    assert_eq!(
        conflict.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{conflict}"
    );
    assert_eq!(
        invitation_row(owner_config, invitation_id)?,
        (1, Some(req_1_id.as_bytes().to_vec()))
    );
    let fingerprint =
        api::command_fingerprint(&api::decode_command(&serde_json::to_vec(redeem_body)?)?)?;
    assert_eq!(
        candidate.runtime.handle().mutation_replay(req_1_id).await?,
        Some(ma2a_store::MutationReplayState::Pending(fingerprint))
    );
    let in_flight_retryable: u8 = rusqlite::Connection::open(candidate.config.database_path())?
        .query_row(
            "SELECT retryable FROM local_mutation_fences WHERE request_id = ?1",
            [req_1_id.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
    assert_eq!(in_flight_retryable, 1);
    Ok(())
}

async fn assert_post_crash_recovery(
    (owner, owner_config): (&Runtime, &StoreConfig),
    (cand_client, cand_config): (&LocalApiClient, &StoreConfig),
    (space, redeem_body, changed_body): (ma2a_core::SpaceId, serde_json::Value, serde_json::Value),
) -> TestResult {
    let post_crash_conflict = call_json(cand_client, changed_body.clone()).await?;
    assert_eq!(
        post_crash_conflict
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{post_crash_conflict}"
    );

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
    let post_transient_conflict = call_json(cand_client, changed_body).await?;
    assert_eq!(
        post_transient_conflict
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{post_transient_conflict}"
    );

    owner.handle().clear_background_faults();
    owner.handle().publish_address().await?;
    let recovered = call_json(cand_client, redeem_body).await?;
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
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn initial_redemption_is_exclusive_in_flight_and_recovers_after_crash() -> TestResult {
    let owner = ServedRuntime::start("enroll-ipc-crash-owner").await?;
    let owner_id = owner.runtime.handle().status().await?.endpoint_id();
    let (_space_hex, ticket_str, ticket) = create_space_and_invite(
        &owner,
        "ipc-crash",
        (
            "71717171717171717171717171717171",
            "72727272727272727272727272727272",
        ),
    )
    .await?;
    let space = ticket.space_id();
    let owner_config = owner.config.clone();
    let bind_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let owner_state = owner.stop_preserving_state().await?;

    let (holding_owner, committed, release) =
        start_holding_owner(&owner_state, owner_config.clone(), (owner_id, bind_port)).await?;

    let candidate = ServedRuntime::start("enroll-ipc-crash-cand").await?;
    let req_1 = "73737373737373737373737373737373";
    let req_1_id = RequestId::try_from([0x73; 16].as_slice())?;
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

    let client_clone = candidate.client.clone();
    let body_clone = redeem_body.clone();
    let in_flight_task = tokio::spawn(async move { call_json(&client_clone, body_clone).await });
    timeout(PROMPT, committed.notified()).await?;

    assert_in_flight_exclusive_and_retryable(
        &candidate,
        (&owner_config, &ticket.invitation_id()),
        (req_1_id, &redeem_body, &changed_body),
    )
    .await?;

    in_flight_task.abort();
    release.cancel();
    holding_owner.shutdown().await?;
    drop(holding_owner);
    let cand_config = candidate.config.clone();
    let cand_state = candidate.stop_preserving_state().await?;

    let owner = Runtime::start(owner_config.clone()).await?;
    let candidate = Runtime::start(cand_config.clone()).await?;
    let (cand_server, cand_client) = serve_runtime(&cand_state, &candidate).await?;

    assert_post_crash_recovery(
        (&owner, &owner_config),
        (&cand_client, &cand_config),
        (space, redeem_body, changed_body),
    )
    .await?;

    cand_server.cancel().await?;
    timeout(PROMPT, candidate.shutdown()).await??;
    timeout(PROMPT, owner.shutdown()).await??;
    drop(cand_state);
    drop(owner_state);
    Ok(())
}
