//! Authenticated Runtime-backed Web mutation integration coverage.

#[path = "support/web.rs"]
#[allow(dead_code, reason = "this integration target uses shared Web fixtures")]
mod support;

use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::{
    body::{Body, to_bytes},
    extract::connect_info::MockConnectInfo,
    http::{Request, StatusCode},
};
use ma2a_runtime::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiClient, LocalApiServer},
    web::{
        PasswordAction, WebAssets, WebAuthConfig, WebRuntimeDependencies, WebServerConfig,
        build_runtime_router,
    },
};
use ma2a_store::StoreConfig;
use support::{TempState, clock};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use zeroize::Zeroizing;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy)]
struct MutationAuth<'a> {
    cookie: Option<&'a str>,
    csrf: Option<&'a str>,
}

impl<'a> MutationAuth<'a> {
    const fn csrf_only(csrf: &'a str) -> Self {
        Self {
            cookie: None,
            csrf: Some(csrf),
        }
    }

    const fn cookie_only(cookie: &'a str) -> Self {
        Self {
            cookie: Some(cookie),
            csrf: None,
        }
    }

    const fn complete(cookie: &'a str, csrf: &'a str) -> Self {
        Self {
            cookie: Some(cookie),
            csrf: Some(csrf),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runtime_mutation_requires_authentication_csrf_and_typed_json() -> TestResult {
    // Given
    let state = TempState::new("runtime-mutation")?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let control = CurrentUserRuntime::open_at(
        state.path(),
        clock(1_000) as Arc<dyn ma2a_runtime::web::Clock>,
        WebAuthConfig::default(),
    )
    .await?;
    control
        .web_auth()
        .change_password(
            PasswordAction::Set,
            Zeroizing::new("web-runtime-passphrase-9!".to_owned()),
        )
        .await?;
    let session = control
        .web_auth()
        .login(Zeroizing::new("web-runtime-passphrase-9!".to_owned()))
        .await?;
    let paths = IpcPaths::new(state.path())?;
    let handle = runtime.handle();
    let bound = LocalApiServer::bind_for_launch(paths.clone(), handle, control.clone(), None);
    let cancellation = CancellationToken::new();
    let api_task = tokio::spawn(bound.await?.serve(cancellation.child_token()));
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
    let unavailable_router = build_runtime_router(
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            LocalApiClient::new(IpcPaths::new(state.path().join("unavailable"))?),
        ),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_345))));
    let cookie = format!(
        "{}={}",
        control.web_auth().session_cookie_name(),
        session.bearer()
    );
    let command = br#"{"version":1,"operation":"session_revoke_all","request_id":"00112233445566778899aabbccddeeff"}"#;

    // When
    let unauthorized = mutation_request(
        &router,
        command,
        MutationAuth::csrf_only(session.csrf_token()),
    )
    .await?;
    let missing_csrf =
        mutation_request(&router, command, MutationAuth::cookie_only(&cookie)).await?;
    let malformed = mutation_request(
        &router,
        br#"{"version":1,"operation":"session_revoke_all"}"#,
        MutationAuth::complete(&cookie, session.csrf_token()),
    )
    .await?;
    let unavailable = mutation_request(
        &unavailable_router,
        command,
        MutationAuth::complete(&cookie, session.csrf_token()),
    )
    .await?;
    let accepted = mutation_request(
        &router,
        command,
        MutationAuth::complete(&cookie, session.csrf_token()),
    )
    .await?;

    // Then
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);
    assert_eq!(malformed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(accepted.status(), StatusCode::OK);
    assert_expired_cookies(accepted.headers(), control.web_auth());
    let response: serde_json::Value =
        serde_json::from_slice(&to_bytes(accepted.into_body(), 65_536).await?)?;
    assert_eq!(
        response
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("sessions_revoked")
    );

    cancellation.cancel();
    api_task.await??;
    runtime.shutdown().await?;
    Ok(())
}

async fn mutation_request(
    router: &axum::Router,
    body: &'static [u8],
    auth: MutationAuth<'_>,
) -> TestResult<axum::response::Response> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/v1/sessions/revoke-all")
        .header("host", "127.0.0.1:43210")
        .header("origin", "http://127.0.0.1:43210")
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/json");
    if let Some(cookie) = auth.cookie {
        request = request.header("cookie", cookie);
    }
    if let Some(csrf) = auth.csrf {
        request = request.header("x-csrf-token", csrf);
    }
    Ok(router
        .clone()
        .oneshot(request.body(Body::from(body))?)
        .await?)
}

fn assert_expired_cookies(
    headers: &axum::http::HeaderMap,
    auth: &ma2a_runtime::web::WebAuthService,
) {
    let cookies = headers.get_all("set-cookie");
    assert!(cookies.iter().any(|value| {
        value
            == &format!(
                "{}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0",
                auth.session_cookie_name()
            )
    }));
    assert!(cookies.iter().any(|value| {
        value
            == &format!(
                "{}=; Path=/; SameSite=Strict; Max-Age=0",
                auth.csrf_cookie_name()
            )
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_revoked_while_the_body_arrives_dispatches_nothing() -> TestResult {
    // Given: a Runtime client that reaches no Runtime, so any dispatch answers 503.
    let state = TempState::new("runtime-mutation-revoked")?;
    let auth = ma2a_runtime::web::WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(1_000) as Arc<dyn ma2a_runtime::web::Clock>,
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(
        PasswordAction::Set,
        Zeroizing::new("web-runtime-passphrase-9!".to_owned()),
    )
    .await?;
    let session = auth
        .login(Zeroizing::new("web-runtime-passphrase-9!".to_owned()))
        .await?;
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            auth.clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            LocalApiClient::new(IpcPaths::new(state.path().join("unavailable"))?),
        ),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_345))));
    let (body, chunks) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(1);
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/sessions/revoke-all")
        .header("host", "127.0.0.1:43210")
        .header("origin", "http://127.0.0.1:43210")
        .header("sec-fetch-site", "same-origin")
        .header("content-type", "application/json")
        .header(
            "cookie",
            format!("{}={}", auth.session_cookie_name(), session.bearer()),
        )
        .header("x-csrf-token", session.csrf_token())
        .body(Body::from_stream(
            tokio_stream::wrappers::ReceiverStream::new(chunks),
        ))?;
    let response = tokio::spawn(router.oneshot(request));

    // When: the handler reads the body only after admitting the session; once it has taken the
    // first half, the session is revoked, and only then does the rest arrive.
    body.send(Ok(
        br#"{"version":1,"operation":"session_revoke_all","#.to_vec()
    ))
    .await?;
    drop(body.reserve().await?);
    auth.revoke_all_sessions().await?;
    body.send(Ok(
        br#""request_id":"00112233445566778899aabbccddeeff"}"#.to_vec()
    ))
    .await?;
    drop(body);

    // Then
    assert_eq!(response.await??.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}
