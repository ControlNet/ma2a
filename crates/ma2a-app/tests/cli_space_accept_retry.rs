//! Process-level CLI retry coverage for `ma2a space accept --request-id`.

use std::{io::Write as _, net::Ipv4Addr, path::Path, process::Stdio};

use iroh::{
    Endpoint, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_tickets::Ticket as _;
use ma2a_core::{EndpointId, RequestId, SignedInviteTicket};
use ma2a_store::{
    AuthorizedEnrollmentRedemption, EnrollmentRedemption, KeyKind, KeyReference, KeyStore,
    Repository, StoreConfig,
};

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult};

#[derive(Clone, Debug)]
struct CommitThenDropReply {
    owner_config: StoreConfig,
    owner_id: EndpointId,
}

fn decode_attempt(
    bytes: &[u8],
    endpoint_id: EndpointId,
) -> Result<(SignedInviteTicket, EnrollmentRedemption), std::io::Error> {
    let ticket_len = usize::from(u16::from_be_bytes(
        bytes
            .get(..2)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| std::io::Error::other("short ticket len"))?,
    ));
    let ticket_end = 2 + ticket_len;
    let ticket = SignedInviteTicket::decode_bytes(
        bytes
            .get(2..ticket_end)
            .ok_or_else(|| std::io::Error::other("short ticket"))?,
    )
    .map_err(std::io::Error::other)?;
    let req_end = ticket_end + 16;
    let request_id = RequestId::try_from(
        bytes
            .get(ticket_end..req_end)
            .ok_or_else(|| std::io::Error::other("short request id"))?,
    )
    .map_err(|_| std::io::Error::other("invalid request id"))?;
    let name_len = usize::from(u16::from_be_bytes(
        bytes
            .get(req_end..req_end + 2)
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| std::io::Error::other("short name len"))?,
    ));
    let display_name = std::str::from_utf8(
        bytes
            .get(req_end + 2..req_end + 2 + name_len)
            .ok_or_else(|| std::io::Error::other("short display name"))?,
    )
    .map_err(std::io::Error::other)?
    .to_owned();
    Ok((
        ticket,
        EnrollmentRedemption {
            endpoint_id,
            request_id,
            display_name,
        },
    ))
}

impl ProtocolHandler for CommitThenDropReply {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote: EndpointId = connection.remote_id().into();
        let (_send, mut receive) = connection.accept_bi().await?;
        let request = receive
            .read_to_end(4_096)
            .await
            .map_err(std::io::Error::other)?;
        let (ticket, redemption) = decode_attempt(&request, remote)?;
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
        connection.close(1_u8.into(), b"lost reply");
        Ok(())
    }
}

async fn start_dropping_owner(
    state_dir: &Path,
    owner_config: StoreConfig,
    (owner_id, bind_port): (EndpointId, u16),
) -> Result<Router, Box<dyn std::error::Error + Send + Sync>> {
    let protected = KeyStore::open(state_dir)?.read(
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

fn pipe_accept(
    fixture: &DaemonFixture,
    args: &[&str],
    ticket: &str,
) -> Result<std::process::Output, Box<dyn std::error::Error + Send + Sync>> {
    let mut child = fixture.command(args).stdin(Stdio::piped()).spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing child stdin")?
        .write_all(format!("{ticket}\n").as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn extract_retry_request_id(stderr: &str) -> Option<String> {
    let marker = "--request-id ";
    let rest = stderr.split_once(marker)?.1;
    let token = rest.split_whitespace().next()?;
    (token.len() == 32
        && token
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')))
    .then(|| token.to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_accept_exposes_request_id_on_uncertain_reply_and_retries_cleanly() -> TestResult {
    let mut owner = DaemonFixture::running("cli-accept-retry-owner")?;
    let _created = owner.run_ok(&["space", "create", "retry-space", "--json"])?;
    let ticket = String::from_utf8(owner.run_ok(&["space", "invite", "retry-space"])?.stdout)?
        .trim()
        .to_owned();
    let parsed_ticket = SignedInviteTicket::decode_string(&ticket)?;
    let space_id = parsed_ticket.space_id();
    let owner_id = parsed_ticket.creator();
    let owner_config = StoreConfig::new(owner.state_dir());

    owner.stop_owned()?;
    let bind_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let dropping_owner = start_dropping_owner(
        owner.state_dir(),
        owner_config.clone(),
        (owner_id, bind_port),
    )
    .await?;

    let candidate = DaemonFixture::running("cli-accept-retry-cand")?;
    let first = pipe_accept(&candidate, &["space", "accept"], &ticket)?;
    assert!(!first.status.success());
    let first_stderr = String::from_utf8(first.stderr)?;
    let request_id = extract_retry_request_id(&first_stderr).ok_or_else(|| {
        format!("retry diagnostic must expose --request-id <32-hex>: {first_stderr}")
    })?;
    assert!(
        !first_stderr.contains(&ticket),
        "invite ticket must never appear in stderr"
    );

    let generation_after_first = Repository::open(&owner_config)?
        .load_space_chain(space_id)?
        .ok_or("owner chain missing")?
        .latest_generation();
    assert_eq!(generation_after_first, 1);

    dropping_owner.shutdown().await?;
    drop(dropping_owner);
    owner.start_owned()?;

    let retried = pipe_accept(
        &candidate,
        &["space", "accept", "--request-id", &request_id],
        &ticket,
    )?;
    assert!(
        retried.status.success(),
        "exact retry failed: {}",
        String::from_utf8_lossy(&retried.stderr)
    );
    let retried_stdout = String::from_utf8(retried.stdout)?;
    assert!(retried_stdout.contains("Joined Space"), "{retried_stdout}");

    let spaces = candidate.json(&["space", "list", "--json"])?;
    assert_eq!(
        spaces
            .pointer("/result/payload")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(1),
        "{spaces}"
    );
    owner.stop_owned()?;
    let generation_after_retry = Repository::open(&owner_config)?
        .load_space_chain(space_id)?
        .ok_or("owner chain missing")?
        .latest_generation();
    assert_eq!(generation_after_retry, generation_after_first);

    candidate.shutdown()?;
    owner.shutdown()
}
