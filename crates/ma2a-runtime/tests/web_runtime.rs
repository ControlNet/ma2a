//! Authenticated Web projection integration coverage.

#[path = "support/web.rs"]
#[allow(
    dead_code,
    reason = "this integration target uses only the shared manual clock"
)]
mod support;

use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_runtime::{
    Runtime,
    api::Command,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{
        WebAssets, WebAuthConfig, WebRuntimeDependencies, WebServerConfig, build_runtime_router,
    },
};
use ma2a_store::StoreConfig;
use support::{TempState, clock};
use tokio_stream::StreamExt as _;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    clippy::too_many_lines,
    reason = "one live daemon scenario proves authentication, connection limits, permit release, revocation, and teardown"
)]
async fn authenticated_snapshot_and_stale_sse_use_the_daemon_projection() -> TestResult {
    // Given
    let state = TempState::new("runtime")?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let auth_clock = clock(1_000);
    let control = CurrentUserRuntime::open_at(
        state.path(),
        Arc::clone(&auth_clock) as Arc<dyn ma2a_runtime::web::Clock>,
        WebAuthConfig::default(),
    )
    .await?;
    control
        .web_auth()
        .change_password(
            ma2a_runtime::web::PasswordAction::Set,
            Zeroizing::new("web-runtime-passphrase-9!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("web-runtime-passphrase-9!".to_owned()))
        .await?;
    let paths = IpcPaths::new(state.path())?;
    let api_server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control.clone(), None)
            .await?;
    let cancellation = CancellationToken::new();
    let api_task = tokio::spawn(api_server.serve(cancellation.child_token()));
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            LocalApiClient::new(paths),
        ),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_345))));
    let cookie = format!("ma2a_session={}", session.bearer());

    // When
    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43210")
                .body(Body::empty())?,
        )
        .await?;
    let snapshot = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43210")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    let stale_events = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/events?since=0")
                .header("host", "127.0.0.1:43210")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;

    // Then
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(snapshot.status(), StatusCode::OK);
    let snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(snapshot.into_body(), 65_536).await?)?;
    let revision = snapshot
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("snapshot revision missing")?;
    assert_eq!(stale_events.status(), StatusCode::OK);
    let events = to_bytes(stale_events.into_body(), 16_384).await?;
    assert!(
        events
            .windows(22)
            .any(|window| window == b"event: resync-required")
    );

    let mut streams = Vec::new();
    for _ in 0..8 {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/events?since={revision}"))
                    .header("host", "127.0.0.1:43210")
                    .header("cookie", &cookie)
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        streams.push(response);
    }
    let limited = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/events?since={revision}"))
                .header("host", "127.0.0.1:43210")
                .header("cookie", &cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);

    drop(streams.pop());
    let replacement = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/v1/events?since={revision}"))
                        .header("host", "127.0.0.1:43210")
                        .header("cookie", &cookie)
                        .body(Body::empty())?,
                )
                .await?;
            if response.status() == StatusCode::OK {
                return TestResult::Ok(response);
            }
            tokio::task::yield_now().await;
        }
    })
    .await??;
    streams.push(replacement);

    control.send(Command::session_revoke_all()?).await?;
    let revoked = streams.pop().ok_or("active event stream missing")?;
    let mut revoked_body = revoked.into_body().into_data_stream();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), revoked_body.next())
            .await?
            .is_none()
    );
    drop(streams);

    let expiring_session = control
        .web_auth()
        .login(Zeroizing::new("web-runtime-passphrase-9!".to_owned()))
        .await?;
    let expiring_cookie = format!("ma2a_session={}", expiring_session.bearer());
    let expiring_snapshot = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/snapshot")
                .header("host", "127.0.0.1:43210")
                .header("cookie", &expiring_cookie)
                .body(Body::empty())?,
        )
        .await?;
    let expiring_snapshot: serde_json::Value =
        serde_json::from_slice(&to_bytes(expiring_snapshot.into_body(), 65_536).await?)?;
    let expiring_revision = expiring_snapshot
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or("expiring snapshot revision missing")?;
    let expiring_stream = router
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/events?since={expiring_revision}"))
                .header("host", "127.0.0.1:43210")
                .header("cookie", &expiring_cookie)
                .body(Body::empty())?,
        )
        .await?;
    assert_eq!(expiring_stream.status(), StatusCode::OK);
    let mut expiring_body = expiring_stream.into_body().into_data_stream();

    auth_clock.set(1_000 + WebAuthConfig::ABSOLUTE_TIMEOUT_MS);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), expiring_body.next())
            .await?
            .is_none()
    );

    cancellation.cancel();
    api_task.await??;
    runtime.shutdown().await?;
    Ok(())
}

/// Synthetic signed fixtures exercise the real Store, IPC codec, HTTP route and SSE stream.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[expect(
    clippy::too_many_lines,
    reason = "one regression covers the complete authenticated transport path"
)]
async fn full_spaces_keep_snapshot_details_and_events_within_the_frame_budget() -> TestResult {
    let state = TempState::new("full-spaces")?;
    let config = StoreConfig::new(state.path());
    let initial = Runtime::start(config.clone()).await?;
    let local = initial.handle().status().await?.endpoint_id();
    initial.shutdown().await?;
    let ids = seed_full_spaces(&config, local)?;
    let runtime = Runtime::start(config).await?;
    let auth_clock = clock(1_000);
    let control =
        CurrentUserRuntime::open_at(state.path(), auth_clock, WebAuthConfig::default()).await?;
    control
        .web_auth()
        .change_password(ma2a_runtime::web::PasswordAction::Set, support::password())
        .await?;
    let session = control.web_auth().login(support::password()).await?;
    let cookie = format!("ma2a_session={}", session.bearer());
    let paths = IpcPaths::new(state.path())?;
    let api_server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let cancellation = CancellationToken::new();
    let api_task = tokio::spawn(api_server.serve(cancellation.child_token()));
    let client = LocalApiClient::new(paths);
    let bytes = client
        .call(&Command::snapshot_fetch())
        .await
        .map_err(|error| format!("full snapshot: {error}"))?;
    assert!(bytes.len() <= ma2a_runtime::api::MAX_LOCAL_RESPONSE_BYTES);
    let snapshot: serde_json::Value = serde_json::from_slice(&bytes)?;
    let spaces = snapshot
        .pointer("/result/payload/spaces")
        .ok_or("missing fixture field")?
        .as_array()
        .ok_or("missing spaces")?;
    assert_eq!(spaces.len(), ids.len());
    for space in spaces {
        assert_eq!(
            space
                .pointer("/member_count")
                .ok_or("missing fixture field")?,
            64
        );
        assert!(space.get("members").is_none());
    }
    let auth =
        CurrentUserRuntime::open_at(state.path(), clock(1_000), WebAuthConfig::default()).await?;
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            auth.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            client.clone(),
        ),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_345))));
    let response = router
        .clone()
        .oneshot(web_read("/api/v1/snapshot", Some(&cookie))?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let payload: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 65_536).await?)?;
    assert_eq!(
        payload.pointer("/spaces").ok_or("missing fixture field")?,
        snapshot
            .pointer("/result/payload/spaces")
            .ok_or("missing fixture field")?
    );
    let first = spaces.first().ok_or("missing first Space")?;
    let id = first
        .pointer("/space_id")
        .ok_or("missing fixture field")?
        .as_str()
        .ok_or("missing Space ID")?;
    let uri = format!("/api/v1/spaces/{id}");
    let denied = router.clone().oneshot(web_read(&uri, None)?).await?;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let detail = router
        .clone()
        .oneshot(web_read(&uri, Some(&cookie))?)
        .await?;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: serde_json::Value =
        serde_json::from_slice(&to_bytes(detail.into_body(), 65_536).await?)?;
    assert_eq!(
        detail.pointer("/space").ok_or("missing fixture field")?,
        first
    );
    let members = detail
        .pointer("/members")
        .ok_or("missing fixture field")?
        .as_array()
        .ok_or("missing detail members")?;
    assert_eq!(members.len(), 64);
    assert!(members.iter().any(|member| {
        member
            .pointer("/label")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|label| label.len() == 64)
    }));
    let mut old_payload = snapshot
        .pointer("/result/payload")
        .ok_or("missing payload")?
        .clone();
    for space in old_payload
        .get_mut("spaces")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("missing spaces")?
    {
        space
            .as_object_mut()
            .ok_or("invalid Space")?
            .insert("members".to_owned(), serde_json::json!(members));
    }
    let old_bytes = serde_json::to_vec(&old_payload)?.len();
    assert!(old_bytes > ma2a_runtime::api::MAX_LOCAL_RESPONSE_BYTES);
    eprintln!(
        "16 full Spaces: old inline payload {old_bytes} bytes; summary IPC response {} bytes; single detail {} bytes",
        bytes.len(),
        serde_json::to_vec(&detail)?.len()
    );
    let absent = router
        .clone()
        .oneshot(web_read(
            &format!("/api/v1/spaces/{}", "ff".repeat(32)),
            Some(&cookie),
        )?)
        .await?;
    assert_eq!(absent.status(), StatusCode::NOT_FOUND);
    let stamp_bytes = client
        .call(&Command::snapshot_stamp())
        .await
        .map_err(|error| format!("stamp: {error}"))?;
    assert!(stamp_bytes.len() < 512);
    let stamp: serde_json::Value = serde_json::from_slice(&stamp_bytes)?;
    assert_eq!(
        stamp
            .pointer("/result/payload/runtime_boot_id")
            .ok_or("missing fixture field")?,
        snapshot
            .pointer("/runtime_boot_id")
            .ok_or("missing fixture field")?
    );
    let revision = stamp
        .pointer("/revision")
        .ok_or("missing fixture field")?
        .as_u64()
        .ok_or("missing stamp revision")?;
    let response = router
        .oneshot(web_read(
            &format!("/api/v1/events?since={revision}"),
            Some(&cookie),
        )?)
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let mut events = response.into_body().into_data_stream();
    // Session creation advances durable revision without loading or changing any Space.
    let _another_session = auth.web_auth().login(support::password()).await?;
    let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.next())
        .await?
        .ok_or("event stream closed")??;
    let event = std::str::from_utf8(&event)?;
    assert!(event.contains("snapshot_invalidated") || event.contains("resync-required"));
    drop(events);
    cancellation.cancel();
    api_task.await??;
    runtime.shutdown().await?;
    Ok(())
}

fn web_read(uri: &str, cookie: Option<&str>) -> TestResult<Request<Body>> {
    let mut request = Request::builder()
        .uri(uri)
        .header("host", "127.0.0.1:43210");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    Ok(request.body(Body::empty())?)
}

fn seed_full_spaces(
    config: &StoreConfig,
    local: ma2a_core::EndpointId,
) -> TestResult<Vec<ma2a_core::SpaceId>> {
    use ma2a_core::{MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, SpacePolicyV1};
    use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation};
    let mut repository = Repository::open(config)?;
    let owner = SpaceMemberV1::new(
        local,
        "local".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let mut members = vec![owner.clone()];
    for _ in 1..64 {
        let peer = iroh::SecretKey::generate().public().into();
        // Quotes and backslashes are valid signed labels and expand when encoded as JSON.
        members.push(SpaceMemberV1::new(
            peer,
            "\"\\".repeat(32),
            MemberCapabilities::new(true, false),
        )?);
    }
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let mut ids = Vec::new();
    for _ in 0..16 {
        let created = repository.create_owned_space(&SpaceCreation::new(
            10,
            owner.clone(),
            SpacePolicyV1::phase_one_default(),
        ))?;
        repository.advance_owned_space(&OwnedSpaceUpdate::new(
            created.space_id(),
            11,
            SpaceManifestMembership::new(members.clone(), Vec::new()),
        ))?;
        let detail = repository
            .space_details(local, created.space_id())?
            .ok_or("missing seeded detail")?;
        assert_eq!(detail.members.len(), 64);
        assert_eq!(detail.revision, repository.revision()?);
        let outsider = iroh::SecretKey::generate().public().into();
        assert!(
            repository
                .space_details(outsider, created.space_id())?
                .is_none()
        );
        ids.push(created.space_id());
    }
    Ok(ids)
}
