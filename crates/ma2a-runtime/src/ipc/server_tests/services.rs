use serde_json::{Value, json};

use super::*;

async fn call(client: &LocalApiClient, request: Value) -> TestResult<Value> {
    let command = api::decode_command(&serde_json::to_vec(&request)?)?;
    Ok(serde_json::from_slice(&client.call(&command).await?)?)
}

fn text<'a>(response: &'a Value, pointer: &str) -> Option<&'a str> {
    response.pointer(pointer).and_then(Value::as_str)
}

fn root_exposure(request_id: &str, space: &str, path: &str) -> Value {
    json!({
        "version": 1, "operation": "service_expose", "request_id": request_id, "space_id": space,
        "kind": "files_root", "root": "docs", "path": path, "access": "read_only",
        "host": null, "port_start": null, "port_end": null,
    })
}

struct Harness {
    runtime: Runtime,
    live_server: LiveServer,
    client: LocalApiClient,
    space: String,
    local: String,
    _state: TempState,
}

impl Harness {
    async fn start(name: &str) -> TestResult<Self> {
        let state = TempState::new_named(name)?;
        let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
        let handle = runtime.handle();
        let space = api::encode_hex(
            handle
                .create_owned_space("svc".to_owned())
                .await?
                .as_bytes(),
        );
        let local = api::encode_hex(handle.status().await?.endpoint_id().as_bytes());
        let control = CurrentUserRuntime::open_at(
            &state.0,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        let paths = IpcPaths::new(&state.0)?;
        let server = LocalApiServer::bind_for_launch(paths.clone(), handle, control, None).await?;
        Ok(Self {
            runtime,
            live_server: LiveServer::spawn(server),
            client: LocalApiClient::new(paths),
            space,
            local,
            _state: state,
        })
    }

    async fn stop(self) -> TestResult {
        assert_eq!(self.live_server.cancel().await?, ServerExit::Cancelled);
        self.runtime.shutdown().await?;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exposures_are_canonical_bound_once_and_removed_exactly() -> TestResult {
    let harness = Harness::start("services-exposure").await?;
    let (client, space) = (&harness.client, harness.space.as_str());
    let exported = TempState::new_named("services-root")?;
    let other = TempState::new_named("services-other")?;
    let exported_path = exported.0.to_str().ok_or("path")?;

    let exposed = call(
        client,
        root_exposure("20202020202020202020202020202020", space, exported_path),
    )
    .await?;
    assert_eq!(text(&exposed, "/result/type"), Some("service_exposed"));
    let canonical = std::fs::canonicalize(&exported.0)?;
    assert_eq!(text(&exposed, "/result/payload/path"), canonical.to_str());
    let replayed = call(
        client,
        root_exposure("20202020202020202020202020202020", space, exported_path),
    )
    .await?;
    assert_eq!(replayed, exposed);
    let rebound = call(
        client,
        root_exposure(
            "21212121212121212121212121212121",
            space,
            other.0.to_str().ok_or("path")?,
        ),
    )
    .await?;
    assert_eq!(text(&rebound, "/error"), Some("conflict"));
    let relative = call(
        client,
        root_exposure("22222222222222222222222222222222", space, "docs"),
    )
    .await?;
    assert_eq!(text(&relative, "/error"), Some("invalid_input"));
    let proxy = call(client, json!({
        "version": 1, "operation": "service_expose", "request_id": "23232323232323232323232323232323",
        "space_id": space, "kind": "proxy", "root": null, "path": null, "access": null,
        "host": "*", "port_start": 8000, "port_end": 8080,
    }))
    .await?;
    assert_eq!(text(&proxy, "/result/payload/host"), Some("*"));

    let listed = call(
        client,
        json!({"version": 1, "operation": "service_exposure_list"}),
    )
    .await?;
    let kinds = listed
        .pointer("/result/payload")
        .and_then(Value::as_array)
        .ok_or("exposures")?
        .iter()
        .filter_map(|exposure| exposure.get("kind").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(kinds, ["files_root", "proxy"]);

    let unexpose = |request_id: &str| {
        json!({
            "version": 1, "operation": "service_unexpose", "request_id": request_id, "space_id": space,
            "kind": "files_root", "root": "docs", "host": null, "port_start": null, "port_end": null,
        })
    };
    let removed = call(client, unexpose("24242424242424242424242424242424")).await?;
    assert_eq!(text(&removed, "/result/payload/path"), canonical.to_str());
    let missing = call(client, unexpose("25252525252525252525252525252525")).await?;
    assert_eq!(text(&missing, "/error"), Some("not_found"));
    harness.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grants_are_owner_signed_and_operations_start_empty() -> TestResult {
    let harness = Harness::start("services-grant").await?;
    let grant = |request_id: &str, peer: &str| {
        json!({
            "version": 1, "operation": "space_grant", "request_id": request_id,
            "space_id": harness.space, "peer_endpoint_id": peer, "files_read": true,
            "files_write": false, "exec": true, "shell": false, "proxy": false,
        })
    };
    let granted = call(
        &harness.client,
        grant("26262626262626262626262626262626", &harness.local),
    )
    .await?;
    assert_eq!(text(&granted, "/result/type"), Some("space_granted"));
    assert_eq!(
        granted.pointer("/result/payload/files_read"),
        Some(&json!(true))
    );
    assert_eq!(granted.pointer("/result/payload/exec"), Some(&json!(true)));
    assert_eq!(granted.pointer("/result/payload/echo"), Some(&json!(true)));
    let unknown = call(
        &harness.client,
        grant("27272727272727272727272727272727", PEER_ENDPOINT_ID),
    )
    .await?;
    assert_eq!(text(&unknown, "/error"), Some("not_found"));

    let operations = call(
        &harness.client,
        json!({"version": 1, "operation": "operations_status"}),
    )
    .await?;
    assert_eq!(
        operations.pointer("/result/payload"),
        Some(&json!({"active_count": 0, "active": [], "recent_count": 0, "recent": []}))
    );
    harness.stop().await
}
