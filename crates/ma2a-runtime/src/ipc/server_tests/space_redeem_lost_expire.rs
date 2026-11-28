//! A lost first enrollment reply followed by an Expired automatic retry must keep the fence.
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use iroh::{
    Endpoint, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use ma2a_core::{EndpointId, RequestId};
use ma2a_store::{
    AuthorizedEnrollmentRedemption, EnrollmentOutcome, KeyKind, KeyReference, KeyStore, Repository,
};

use super::{
    space_redeem_support::{ServedRuntime, call_json, create_space_and_invite, invitation_row},
    *,
};

#[derive(Clone, Debug)]
struct CommitDropThenExpire {
    owner_config: StoreConfig,
    owner_id: EndpointId,
    attempts: Arc<AtomicUsize>,
}

impl ProtocolHandler for CommitDropThenExpire {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let remote: EndpointId = connection.remote_id().into();
        let (mut send, mut receive) = connection.accept_bi().await?;
        let request = receive
            .read_to_end(4_096)
            .await
            .map_err(std::io::Error::other)?;
        let (ticket, redemption) = crate::enrollment::decode_attempt(&request, remote)
            .map_err(|_| std::io::Error::other("decode attempt failed"))?;
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        let now_ms = if attempt == 0 {
            i64::try_from(ticket.created_at_ms()).map_err(std::io::Error::other)? + 1
        } else {
            i64::try_from(ticket.expires_at_ms()).map_err(std::io::Error::other)? + 1
        };
        let authorized = AuthorizedEnrollmentRedemption::new(ticket, redemption, now_ms)
            .requiring_owner_address(self.owner_id);
        let mut repo = Repository::open(&self.owner_config).map_err(std::io::Error::other)?;
        let outcome = repo
            .redeem_enrollment(&authorized)
            .map_err(std::io::Error::other)?;
        if attempt == 0 {
            connection.close(1_u8.into(), b"lost first reply");
            return Ok(());
        }
        assert!(matches!(outcome, EnrollmentOutcome::Expired));
        send.write_all(&[2, 0, 0])
            .await
            .map_err(std::io::Error::other)?;
        send.finish().map_err(std::io::Error::other)?;
        connection.closed().await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_first_reply_followed_by_expiry_keeps_candidate_fence() -> TestResult {
    let owner = ServedRuntime::start("enroll-lost-expire-owner").await?;
    let owner_id = owner.runtime.handle().status().await?.endpoint_id();
    let (_space_hex, ticket_str, ticket) = create_space_and_invite(
        &owner,
        "lost-expire",
        (
            "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1",
            "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
        ),
    )
    .await?;
    let owner_config = owner.config.clone();
    let bind_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let owner_state = owner.stop_preserving_state().await?;

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
    let router = Router::builder(endpoint)
        .accept(
            ma2a_net::ENROLLMENT_ALPN,
            CommitDropThenExpire {
                owner_config: owner_config.clone(),
                owner_id,
                attempts: Arc::new(AtomicUsize::new(0)),
            },
        )
        .spawn();

    let candidate = ServedRuntime::start("enroll-lost-expire-cand").await?;
    let req_1 = "b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3";
    let req_1_id = RequestId::try_from([0xb3; 16].as_slice())?;
    let first = call_json(
        &candidate.client,
        serde_json::json!({
            "version": 1, "operation": "space_redeem", "request_id": req_1, "invitation": ticket_str,
        }),
    )
    .await?;
    assert_eq!(
        invitation_row(&owner_config, &ticket.invitation_id())?,
        (1, Some(req_1_id.as_bytes().to_vec()))
    );
    assert_ne!(
        first.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::EXPIRED.name()),
        "expired automatic retry must not replace lost-reply uncertainty: {first}"
    );
    let remediation = first
        .get("remediation")
        .and_then(serde_json::Value::as_str)
        .ok_or("lost first reply followed by expiry must retain remote-effect remediation")?;
    assert!(
        remediation.contains("remote") && remediation.contains("retry"),
        "unexpected remediation: {remediation}"
    );
    let conflict = call_json(
        &candidate.client,
        serde_json::json!({
            "version": 1, "operation": "space_create", "request_id": req_1, "name": "reused",
        }),
    )
    .await?;
    assert_eq!(
        conflict.get("error").and_then(serde_json::Value::as_str),
        Some(ProtocolError::CONFLICT.name()),
        "{conflict}"
    );

    router.shutdown().await?;
    candidate.shutdown().await?;
    drop(owner_state);
    Ok(())
}
