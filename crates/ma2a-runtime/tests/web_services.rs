//! Authenticated Web projection of service exposures, operations, and hosted Processes.

#[path = "support/web.rs"]
#[allow(
    dead_code,
    reason = "this integration target uses only the temporary state, clock, and password"
)]
mod support;

use std::{error::Error, net::SocketAddr};

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
        WebAssets, WebAuthConfig, WebRuntimeDependencies, WebServerConfig, build_runtime_router,
    },
};
use ma2a_store::StoreConfig;
use serde_json::{Value, json};
use support::{TempState, clock};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;

type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;

fn web_read(uri: &str, cookie: Option<&str>) -> TestResult<Request<Body>> {
    let mut request = Request::builder()
        .uri(uri)
        .header("host", "127.0.0.1:43210");
    if let Some(cookie) = cookie {
        request = request.header("cookie", cookie);
    }
    Ok(request.body(Body::empty())?)
}

async fn call(client: &LocalApiClient, command: &Value) -> TestResult<Value> {
    let command = ma2a_runtime::api::decode_command(command.to_string().as_bytes())?;
    let bytes = client
        .call(&command)
        .await
        .map_err(|error| format!("local API: {error}"))?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn body(response: axum::response::Response) -> TestResult<Value> {
    Ok(serde_json::from_slice(
        &to_bytes(response.into_body(), 65_536).await?,
    )?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exposures_operations_and_processes_are_readable_only_with_a_session() -> TestResult {
    let state = TempState::new("web-services")?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let control =
        CurrentUserRuntime::open_at(state.path(), clock(1_000), WebAuthConfig::default()).await?;
    control
        .web_auth()
        .change_password(ma2a_runtime::web::PasswordAction::Set, support::password())
        .await?;
    let session = control.web_auth().login(support::password()).await?;
    let cookie = format!("ma2a_session={}", session.bearer());
    let paths = IpcPaths::new(state.path())?;
    let api_server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control.clone(), None)
            .await?;
    let cancellation = CancellationToken::new();
    let api_task = tokio::spawn(api_server.serve(cancellation.child_token()));
    let client = LocalApiClient::new(paths);
    let created = call(
        &client,
        &json!({"version": 1, "operation": "space_create",
            "request_id": "11".repeat(16), "name": "lab"}),
    )
    .await?;
    let space_id = created
        .pointer("/result/payload/space_id")
        .and_then(Value::as_str)
        .ok_or("space_create returned no Space ID")?
        .to_owned();
    call(
        &client,
        &json!({"version": 1, "operation": "service_expose", "request_id": "22".repeat(16),
            "space_id": space_id, "kind": "exec", "root": null, "path": null, "access": null,
            "host": null, "port_start": null, "port_end": null}),
    )
    .await?;
    let router = build_runtime_router(
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            client,
        ),
        WebServerConfig::default(),
        43_210,
    )
    .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12_345))));

    for uri in ["/api/v1/services", "/api/v1/operations", "/api/v1/processes"] {
        let denied = router.clone().oneshot(web_read(uri, None)?).await?;
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
    let exposures = router
        .clone()
        .oneshot(web_read("/api/v1/services", Some(&cookie))?)
        .await?;
    assert_eq!(exposures.status(), StatusCode::OK);
    assert_eq!(
        body(exposures).await?,
        json!([{"space_id": space_id, "kind": "exec", "root": null, "path": null,
            "access": null, "host": null, "port_start": null, "port_end": null}])
    );
    let operations = router
        .clone()
        .oneshot(web_read("/api/v1/operations", Some(&cookie))?)
        .await?;
    assert_eq!(operations.status(), StatusCode::OK);
    assert_eq!(
        body(operations).await?,
        json!({"active_count": 0, "active": [], "recent_count": 0, "recent": []})
    );

    let processes = router
        .clone()
        .oneshot(web_read("/api/v1/processes", Some(&cookie))?)
        .await?;
    assert_eq!(processes.status(), StatusCode::OK);
    assert_eq!(
        body(processes).await?,
        json!({"processes": [], "next": null})
    );

    cancellation.cancel();
    api_task.await??;
    runtime.shutdown().await?;
    Ok(())
}
