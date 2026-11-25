//! Outbound Echo calls and departures waiting on another Endpoint are bounded where the actor
//! admits them, as redemptions are.
//!
//! The exchanges run on real Iroh I/O, so this uses real time, as `departure_test` does.
use std::time::Duration;

use ma2a_core::{EchoError, EndpointId, InviteEntropy, RequestId, SpaceId};
use ma2a_store::StoreConfig;
use tokio::time::timeout;

use super::relay_lifecycle_test::TempState;
use crate::{EnrollmentAttempt, EnrollmentCreation, Runtime, SpaceDepartureErrorCode};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Far below the ten-second Echo and thirty-second departure bounds, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(5);

struct OfflineOwner {
    dirs: [TempState; 2],
    member: Runtime,
    owner: EndpointId,
    spaces: Vec<SpaceId>,
}

/// A member of `count` Spaces whose owner has stopped answering.
async fn member_of_offline_owner(label: &str, count: u8) -> TestResult<OfflineOwner> {
    let dirs = [
        TempState::new(&format!("{label}-owner"))?,
        TempState::new(&format!("{label}-member"))?,
    ];
    let owner = Runtime::start(StoreConfig::new(&dirs[0].0)).await?;
    let member = Runtime::start(StoreConfig::new(&dirs[1].0)).await?;
    let mut spaces = Vec::new();
    let mut joining = tokio::task::JoinSet::new();
    for marker in 0..count {
        let space = owner
            .handle()
            .create_owned_space(format!("{label}-{marker}"))
            .await?;
        let ticket = owner
            .handle()
            .create_enrollment_invite(EnrollmentCreation::new(
                space,
                300_000,
                InviteEntropy::random()?,
            )?)
            .await?;
        let attempt = EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([marker; 16].as_slice())?,
            "member".to_owned(),
        );
        let handle = member.handle();
        joining.spawn(async move { handle.redeem_enrollment(attempt).await });
        spaces.push(space);
        // Stay inside the bounds on redemptions pending at the member and on enrollment
        // connections one peer may hold at the owner.
        if joining.len() == 4 {
            while let Some(joined) = joining.join_next().await {
                joined??;
            }
        }
    }
    while let Some(joined) = joining.join_next().await {
        joined??;
    }
    let owner_id = owner.handle().status().await?.endpoint_id;
    owner.shutdown().await?;
    Ok(OfflineOwner {
        dirs,
        member,
        owner: owner_id,
        spaces,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_echo_past_the_pending_bound_is_refused_at_once() -> TestResult {
    // Given
    let fixture = member_of_offline_owner("echo-bound", 1).await?;

    // When: as many Echo calls as are allowed wait on the owner, and one more is made.
    let mut pending = Vec::new();
    for marker in 0..u8::try_from(super::echo::MAX_PENDING_OUTBOUND_ECHOES)? {
        let handle = fixture.member.handle();
        let owner = fixture.owner;
        pending.push(tokio::spawn(async move {
            handle
                .echo(RequestId::try_from([marker; 16].as_slice())?, owner, b"hi")
                .await
                .map_err(Box::<dyn std::error::Error + Send + Sync>::from)
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let refused = timeout(
        PROMPT,
        fixture.member.handle().echo(
            RequestId::try_from([0xff; 16].as_slice())?,
            fixture.owner,
            b"hi",
        ),
    )
    .await?;

    // Then: the extra one is refused at once, and the Runtime keeps answering.
    assert!(matches!(refused, Err(EchoError::ConcurrencyExceeded)));
    assert!(pending.iter().all(|echoing| !echoing.is_finished()));
    timeout(PROMPT, fixture.member.handle().status()).await??;
    timeout(PROMPT, fixture.member.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_departure_past_the_pending_bound_is_refused_at_once() -> TestResult {
    // Given
    let bound = crate::departure_actor::MAX_PENDING_DEPARTURES;
    let fixture = member_of_offline_owner("departure-bound", u8::try_from(bound + 1)?).await?;
    let (waiting, extra) = fixture.spaces.split_at(bound);

    // When: as many departures as are allowed wait on the owner, and one more is made.
    let mut pending = Vec::new();
    for (marker, space) in (0_u8..).zip(waiting.iter().copied()) {
        let handle = fixture.member.handle();
        let request_id = RequestId::try_from([marker; 16].as_slice())?;
        pending.push(tokio::spawn(async move {
            handle.leave_space(space, request_id).await
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let space = *extra.first().ok_or("one Space past the bound")?;
    let refused = timeout(
        PROMPT,
        fixture
            .member
            .handle()
            .leave_space(space, RequestId::try_from([0xff; 16].as_slice())?),
    )
    .await?
    .err()
    .ok_or("a departure past the bound must be refused")?;

    let again = timeout(
        PROMPT,
        fixture
            .member
            .handle()
            .leave_space(space, RequestId::try_from([0xfe; 16].as_slice())?),
    )
    .await?
    .err()
    .ok_or("a departure past the bound must be refused")?;

    // Then: the extra one is refused at once as retryable, without claiming its Space, and the
    // Runtime keeps answering.
    assert_eq!(refused.code(), SpaceDepartureErrorCode::BUSY, "{refused}");
    assert_eq!(again.code(), SpaceDepartureErrorCode::BUSY, "{again}");
    assert!(pending.iter().all(|leaving| !leaving.is_finished()));
    timeout(PROMPT, fixture.member.handle().status()).await??;
    timeout(PROMPT, fixture.member.shutdown()).await??;
    Ok(())
}

/// A local Echo the Runtime refuses before sending anything leaves its request ID free.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_echo_past_the_pending_bound_releases_its_request_id() -> TestResult {
    // Given: as many Echo calls as are allowed wait on an owner that stopped answering, and
    // the local API serves the member.
    let fixture = member_of_offline_owner("echo-ipc-bound", 1).await?;
    let state = &fixture.dirs[1].0;
    let control = crate::current_user::CurrentUserRuntime::open_at(
        state,
        std::sync::Arc::new(crate::web::SystemClock::default()),
        crate::web::WebAuthConfig::default(),
    )
    .await?;
    let paths = crate::ipc::IpcPaths::new(state)?;
    let server = crate::ipc::LocalApiServer::bind_for_launch(
        paths.clone(),
        fixture.member.handle(),
        control,
        None,
    )
    .await?;
    let serving = tokio_util::sync::CancellationToken::new();
    let serve = tokio::spawn(server.serve(serving.child_token()));
    let mut pending = Vec::new();
    for marker in 0..u8::try_from(super::echo::MAX_PENDING_OUTBOUND_ECHOES)? {
        let (handle, owner) = (fixture.member.handle(), fixture.owner);
        let request_id = RequestId::try_from([marker; 16].as_slice())?;
        pending.push(tokio::spawn(async move {
            handle.echo(request_id, owner, b"hi").await
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    // When: one more Echo is asked for over the local API.
    let request_id = RequestId::try_from([0xee; 16].as_slice())?;
    let request = serde_json::json!({
        "version": 1, "operation": "echo_call",
        "request_id": crate::api::encode_hex(request_id.as_bytes()),
        "target_endpoint_id": crate::api::encode_hex(fixture.owner.as_bytes()), "payload": "hi",
    });
    let command = crate::api::decode_command(&serde_json::to_vec(&request)?)?;
    let client = crate::ipc::LocalApiClient::new(paths.clone());
    let answer: serde_json::Value =
        serde_json::from_slice(&timeout(PROMPT, client.call(&command)).await??)?;

    // Then: it is refused with no effect, so its request ID is not taken.
    let replay = fixture.member.handle().mutation_replay(request_id).await?;
    serving.cancel();
    serve.await??;
    paths.remove_stale_endpoint()?;
    timeout(PROMPT, fixture.member.shutdown()).await??;
    assert_eq!(
        answer.get("error"),
        Some(&serde_json::json!("conflict")),
        "{answer}"
    );
    assert_eq!(answer.get("remediation"), Some(&serde_json::Value::Null));
    assert_eq!(replay, None);
    drop(pending);
    Ok(())
}
