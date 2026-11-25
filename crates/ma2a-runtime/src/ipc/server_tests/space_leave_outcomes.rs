//! Departure distinguishes pre-admission busy refusals, status 255, response loss, and rejection.
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use iroh::{
    Endpoint, RelayMode,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use ma2a_core::{InviteEntropy, RequestId, SpaceAuthorizationView, SpaceId};
use ma2a_net::{AddressPublishRequest, AddressPublisher, ENROLLMENT_ALPN};
use ma2a_store::{KeyKind, KeyReference, KeyStore, Repository};
use tokio::time::timeout;

use super::*;
use crate::{EnrollmentAttempt, EnrollmentCreation};

const PROMPT: Duration = Duration::from_secs(10);

fn leave_command(space: SpaceId, request: &str) -> TestResult<api::Command> {
    let space_hex = api::encode_hex(space.as_bytes());
    Ok(api::decode_command(
        format!(
            r#"{{"version":1,"operation":"space_leave","request_id":"{request}","space_id":"{space_hex}"}}"#
        )
        .as_bytes(),
    )?)
}

async fn minimal_endpoint() -> TestResult<Endpoint> {
    Ok(Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind()
        .await?)
}

async fn enroll_pair(
    owner_dir: &TempState,
    member_dir: &TempState,
    req_byte: u8,
) -> TestResult<(Runtime, Runtime, SpaceId)> {
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("leave-outcomes".to_owned())
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
            RequestId::try_from([req_byte; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    Ok((owner, member, space))
}

async fn serve_member(
    dir: &TempState,
    member: &Runtime,
) -> TestResult<(LiveServer, LocalApiClient)> {
    let paths = IpcPaths::new(&dir.0)?;
    let control = CurrentUserRuntime::open_at(
        &dir.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), member.handle(), control, None).await?,
    );
    Ok((server, LocalApiClient::new(paths)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_authority_releases_the_departure_request_id_for_retry() -> TestResult {
    // Given: an owner and member enrolled in a Space, with the member served over IPC.
    let owner_state = TempState::new_named("leave-busy-owner")?;
    let member_state = TempState::new_named("leave-busy-member")?;
    let (owner, member, space) = enroll_pair(&owner_state, &member_state, 0xb1).await?;
    let owner_config = StoreConfig::new(&owner_state.0);
    let before_generation = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();
    let (server, client) = serve_member(&member_state, &member).await?;

    // Saturate the owner's 32-connection global enrollment admission limit (4 peers x 8).
    let owner_addr = owner.handle().status().await?.endpoint_addr;
    let mut flooders = Vec::new();
    let mut held = Vec::new();
    for _ in 0..4 {
        let peer = minimal_endpoint().await?;
        for _ in 0..8 {
            held.push(timeout(PROMPT, peer.connect(owner_addr.clone(), ENROLLMENT_ALPN)).await??);
        }
        flooders.push(peer);
    }
    let probe_peer = minimal_endpoint().await?;
    let probe = timeout(PROMPT, probe_peer.connect(owner_addr, ENROLLMENT_ALPN)).await??;
    timeout(PROMPT, probe.closed()).await?;
    probe_peer.close().await;

    // When: the member leaves while the authority is busy.
    let command = leave_command(space, "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2")?;
    let request_id = command.request_id().ok_or("request id missing")?;
    let refused: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&command)).await??)?;

    // Then: the refusal is retryable unavailable with no remediation and no replay fence.
    assert_eq!(
        refused.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{refused}"
    );
    assert_eq!(refused.get("remediation"), Some(&serde_json::Value::Null));
    assert_eq!(member.handle().mutation_replay(request_id).await?, None);

    // And once permits are released, retrying with the same request ID completes once.
    for connection in held {
        connection.close(0_u8.into(), b"");
    }
    for peer in flooders {
        peer.close().await;
    }
    let completed: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&command)).await??)?;
    assert_eq!(completed.get("error"), None, "{completed}");
    assert_eq!(
        completed
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("space_left")
    );
    let after_generation = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?
        .latest_generation();
    assert_eq!(after_generation, before_generation + 1);
    assert!(!member.handle().status().await?.memberships.contains(&space));

    server.cancel().await?;
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}

#[derive(Clone, Debug)]
struct ScriptedDeparture {
    calls: Arc<AtomicUsize>,
    responses: Arc<Vec<Option<u8>>>,
}

impl ProtocolHandler for ScriptedDeparture {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) = connection.accept_bi().await?;
        let _request = receive.read_to_end(4096).await.map_err(io::Error::other)?;
        let index = self.calls.fetch_add(1, Ordering::Relaxed);
        match self.responses.get(index).copied().flatten() {
            Some(status) => {
                send.write_all(&[status, 0, 0])
                    .await
                    .map_err(io::Error::other)?;
                send.finish().map_err(io::Error::other)?;
                connection.closed().await;
            }
            None => connection.close(1_u8.into(), b"lost"),
        }
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_255_and_response_loss_report_unreachable_and_stay_fenced() -> TestResult {
    // Given: a member enrolled in a Space whose authority now runs a scripted handler.
    let owner_state = TempState::new_named("leave-script-owner")?;
    let member_state = TempState::new_named("leave-script-member")?;
    let (owner, member, space) = enroll_pair(&owner_state, &member_state, 0xb3).await?;
    owner.shutdown().await?;

    let protected = KeyStore::open(&owner_state.0)?.read(
        KeyKind::Endpoint,
        &KeyReference::parse("endpoint-identity-v1")?,
    )?;
    let owner_secret = iroh::SecretKey::from_bytes(&<[u8; 32]>::try_from(protected.as_ref())?);
    let calls = Arc::new(AtomicUsize::new(0));
    let mock = Endpoint::builder(presets::Minimal)
        .secret_key(owner_secret.clone())
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .alpns(vec![ENROLLMENT_ALPN.to_vec()])
        .bind()
        .await?;
    let mock_addr = mock.addr();
    let router = Router::builder(mock)
        .accept(
            ENROLLMENT_ALPN,
            ScriptedDeparture {
                calls: Arc::clone(&calls),
                responses: Arc::new(vec![Some(255), None, Some(1)]),
            },
        )
        .spawn();
    let member_config = StoreConfig::new(&member_state.0);
    let mut repo = Repository::open(&member_config)?;
    let chain = repo
        .load_space_chain(space)?
        .ok_or("member chain missing")?;
    let auth = SpaceAuthorizationView::from_chain(&chain);
    let now_ms = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis(),
    )?;
    AddressPublisher::new(owner_secret, mock_addr)?.publish(
        &mut repo,
        AddressPublishRequest::new(&auth, now_ms).force_advance(),
    )?;
    drop(repo);
    member.shutdown().await?;
    let member = Runtime::start(member_config).await?;
    let (server, client) = serve_member(&member_state, &member).await?;

    // When / Then (1): authority status 255 reports unavailable (unknown outcome), not conflict.
    let cmd_255 = leave_command(space, "b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4")?;
    let id_255 = cmd_255.request_id().ok_or("request id missing")?;
    let resp_255: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&cmd_255)).await??)?;
    assert_eq!(
        resp_255.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "status 255 must report unavailable rather than rejected: {resp_255}"
    );
    assert!(member.handle().mutation_replay(id_255).await?.is_some());

    // When / Then (2): actual response loss reports unavailable and stays fenced on retry.
    let cmd_lost = leave_command(space, "b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5")?;
    let id_lost = cmd_lost.request_id().ok_or("request id missing")?;
    let resp_lost: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&cmd_lost)).await??)?;
    assert_eq!(
        resp_lost.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{resp_lost}"
    );
    assert!(member.handle().mutation_replay(id_lost).await?.is_some());
    let replayed: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&cmd_lost)).await??)?;
    assert_eq!(replayed, resp_lost);
    assert_eq!(calls.load(Ordering::Relaxed), 2);

    // When / Then (3): authority status 1 still reports conflict (rejected).
    let cmd_rej = leave_command(space, "b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6b6")?;
    let resp_rej: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&cmd_rej)).await??)?;
    assert_eq!(
        resp_rej.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{resp_rej}"
    );

    server.cancel().await?;
    router.shutdown().await?;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}
