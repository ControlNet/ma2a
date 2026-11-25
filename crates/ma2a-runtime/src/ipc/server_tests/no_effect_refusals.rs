//! Requests refused before anything runs release their request ID: no warning, no replay entry.

use super::*;

/// A Runtime and its local API server over a private state directory.
pub(super) struct Served {
    pub(super) state: TempState,
    pub(super) runtime: Runtime,
    server: LiveServer,
    paths: IpcPaths,
    pub(super) client: LocalApiClient,
}

impl Served {
    pub(super) async fn start() -> TestResult<Self> {
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
    pub(super) async fn refused(
        &self,
        request: &serde_json::Value,
        error: ProtocolError,
    ) -> TestResult {
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

    pub(super) async fn stop(self) -> TestResult {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_refused_before_they_run_have_no_effect() -> TestResult {
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("refusals".to_owned())
        .await?;
    let space_id = api::encode_hex(space.as_bytes());
    let output = served.state.0.join("ticket");
    let output = output.to_str().ok_or("invalid path")?;
    let peer = api::encode_hex(iroh::SecretKey::generate().public().as_bytes());
    let refusals = [
        // An invitation may live at most five minutes.
        (
            serde_json::json!({
                "version": 1, "operation": "space_invite", "request_id": "51".repeat(16),
                "space_id": space_id, "ttl_ms": 600_000, "output_path": output,
            }),
            ProtocolError::INVALID_INPUT,
        ),
        (
            serde_json::json!({
                "version": 1, "operation": "space_leave", "request_id": "52".repeat(16),
                "space_id": "53".repeat(32),
            }),
            ProtocolError::NOT_FOUND,
        ),
        (
            serde_json::json!({
                "version": 1, "operation": "space_redeem", "request_id": "54".repeat(16),
                "invitation": "not-a-ticket",
            }),
            ProtocolError::INVALID_INPUT,
        ),
        // A Space this Runtime does not hold, and a peer that is no member of one it owns.
        (
            serde_json::json!({
                "version": 1, "operation": "space_revoke", "request_id": "57".repeat(16),
                "space_id": "58".repeat(32), "peer_endpoint_id": peer,
            }),
            ProtocolError::NOT_FOUND,
        ),
        (
            serde_json::json!({
                "version": 1, "operation": "space_revoke", "request_id": "59".repeat(16),
                "space_id": space_id, "peer_endpoint_id": peer,
            }),
            ProtocolError::NOT_FOUND,
        ),
        // No Web password is set yet, so there is none to reset.
        (
            serde_json::json!({
                "version": 1, "operation": "ui_password_reset", "request_id": "5a".repeat(16),
                "password": "correct horse battery staple",
            }),
            ProtocolError::CONFLICT,
        ),
        // Native TLS needs both files; external termination takes neither.
        (
            serde_json::json!({
                "version": 1, "operation": "private_relay_configure",
                "request_id": "55".repeat(16), "mode": "native_tls",
                "listen": "127.0.0.1:3340", "public_url": "https://relay.example.invalid/",
                "served_space_ids": [space_id], "certificate_path": null,
                "private_key_path": null,
            }),
            ProtocolError::INVALID_INPUT,
        ),
        (
            serde_json::json!({
                "version": 1, "operation": "private_relay_configure",
                "request_id": "56".repeat(16), "mode": "external_termination",
                "listen": "127.0.0.1:3340", "public_url": "https://relay.example.invalid/",
                "served_space_ids": [space_id], "certificate_path": output,
                "private_key_path": null,
            }),
            ProtocolError::INVALID_INPUT,
        ),
    ];
    for (request, error) in &refusals {
        served.refused(request, *error).await?;
    }
    served.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invitation_retried_after_its_output_path_is_fixed_is_created_once() -> TestResult {
    // Given: the output path is taken.
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("retry".to_owned())
        .await?;
    let output = served.state.0.join("ticket");
    std::fs::write(&output, b"taken")?;
    let request = serde_json::json!({
        "version": 1, "operation": "space_invite", "request_id": "61".repeat(16),
        "space_id": api::encode_hex(space.as_bytes()), "ttl_ms": 60_000,
        "output_path": output.to_str().ok_or("invalid path")?,
    });
    served
        .refused(&request, ProtocolError::INVALID_INPUT)
        .await?;

    // When: the obstruction is removed and the same request is sent again.
    std::fs::remove_file(&output)?;
    let command = api::decode_command(&serde_json::to_vec(&request)?)?;
    let value: serde_json::Value = serde_json::from_slice(&served.client.call(&command).await?)?;

    // Then: it runs, once.
    assert_eq!(value.get("error"), None, "{value}");
    assert!(std::fs::read(&output)?.starts_with(b"ma2a"));
    assert_eq!(invitations(&served)?, 1);
    served.stop().await
}

/// A directory the daemon can search but not add entries to.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invitation_into_an_unwritable_directory_has_no_effect() -> TestResult {
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("unwritable".to_owned())
        .await?;
    let directory = served.state.0.join("read-only");
    std::fs::create_dir(&directory)?;
    set_writable(&directory, false)?;
    let revision = served.runtime.handle().status().await?.revision();
    let request = serde_json::json!({
        "version": 1, "operation": "space_invite", "request_id": "62".repeat(16),
        "space_id": api::encode_hex(space.as_bytes()), "ttl_ms": 60_000,
        "output_path": directory.join("ticket").to_str().ok_or("invalid path")?,
    });

    let refused = served.refused(&request, ProtocolError::INVALID_INPUT).await;

    set_writable(&directory, true)?;
    refused?;
    assert_eq!(served.runtime.handle().status().await?.revision(), revision);
    assert_eq!(invitations(&served)?, 0);
    assert_eq!(std::fs::read_dir(&directory)?.count(), 0);
    served.stop().await
}

/// Lets, or stops, the current account adding entries to `directory`.
fn set_writable(directory: &std::path::Path, writable: bool) -> TestResult {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if writable { 0o700 } else { 0o500 };
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(windows)]
    {
        // A rule that denies everyone adding a file (WD) or a subdirectory (AD).
        let mut icacls = std::process::Command::new("icacls");
        icacls.arg(directory).stdout(std::process::Stdio::null());
        if writable {
            icacls.args(["/remove:d", "*S-1-1-0"]);
        } else {
            icacls.args(["/deny", "*S-1-1-0:(WD,AD)"]);
        }
        let status = icacls.status()?;
        if !status.success() {
            return Err(format!("icacls: {status}").into());
        }
    }
    Ok(())
}

pub(super) fn invitations(served: &Served) -> TestResult<u64> {
    let config = StoreConfig::new(&served.state.0);
    let connection = rusqlite::Connection::open(config.database_path())?;
    Ok(connection.query_row("SELECT COUNT(*) FROM invitations", [], |row| row.get(0))?)
}
