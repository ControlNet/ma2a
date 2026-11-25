//! Requests refused before anything runs release their request ID: no warning, no replay entry.

use super::*;

/// A Runtime and its local API server over a private state directory.
struct Served {
    state: TempState,
    runtime: Runtime,
    server: LiveServer,
    paths: IpcPaths,
    client: LocalApiClient,
}

impl Served {
    async fn start() -> TestResult<Self> {
        let state = TempState::new_named("no-effect-refusals")?;
        let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
        let control = CurrentUserRuntime::open_at(
            &state.0,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        let paths = IpcPaths::new(&state.0)?;
        let server = LiveServer::spawn(
            LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?,
        );
        let client = LocalApiClient::new(paths.clone());
        Ok(Self {
            state,
            runtime,
            server,
            paths,
            client,
        })
    }

    /// Sends `request` and checks it is refused with `error`, as having had no effect.
    async fn refused(&self, request: &serde_json::Value, error: ProtocolError) -> TestResult {
        let command = api::decode_command(&serde_json::to_vec(request)?)?;
        let value: serde_json::Value = serde_json::from_slice(&self.client.call(&command).await?)?;
        assert_eq!(
            value.get("error").and_then(serde_json::Value::as_str),
            Some(error.name()),
            "{request}"
        );
        assert_eq!(value.get("remediation"), Some(&serde_json::Value::Null));
        let request_id = command.request_id().ok_or("request id missing")?;
        assert_eq!(
            self.runtime.handle().mutation_replay(request_id).await?,
            None
        );
        Ok(())
    }

    async fn stop(self) -> TestResult {
        self.server.cancel().await?;
        self.paths.remove_stale_endpoint()?;
        self.runtime.shutdown().await?;
        drop(self.state);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_configuration_the_runtime_cannot_apply_has_no_effect() -> TestResult {
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("relay".to_owned())
        .await?;
    // An external TLS backend must listen on loopback only.
    served
        .refused(
            &serde_json::json!({
                "version": 1, "operation": "private_relay_configure",
                "request_id": "41".repeat(16), "mode": "external_termination",
                "listen": "0.0.0.0:3340", "public_url": "https://relay.example.invalid/",
                "served_space_ids": [api::encode_hex(space.as_bytes())],
                "certificate_path": null, "private_key_path": null,
            }),
            ProtocolError::INVALID_INPUT,
        )
        .await?;
    served.stop().await
}
