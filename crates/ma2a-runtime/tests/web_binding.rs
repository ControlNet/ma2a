//! Dual-stack loopback binding coverage.

#[path = "support/web.rs"]
mod support;

use ma2a_runtime::web::{WebAuthConfig, WebAuthService, WebServerConfig};
use ma2a_store::StoreConfig;
use support::{RunningServer, TempState, TestResult, clock};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ipv6_loopback_uses_the_same_selected_port() -> TestResult {
    // Given
    let state = TempState::new("ipv6")?;
    let auth = WebAuthService::open(
        StoreConfig::new(state.path()),
        clock(5_000),
        WebAuthConfig::default(),
    )
    .await?;
    let server = RunningServer::start(auth, WebServerConfig::default()).await?;

    // When
    let mut stream =
        tokio::net::TcpStream::connect((std::net::Ipv6Addr::LOCALHOST, server.port())).await?;
    stream
        .write_all(
            format!(
                "GET /api/v1/web/auth/state HTTP/1.1\r\nHost: [::1]:{}\r\nConnection: close\r\n\r\n",
                server.port()
            )
            .as_bytes(),
        )
        .await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;

    // Then
    assert!(response.starts_with(b"HTTP/1.1 200"));
    server.stop().await
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mapped_wildcard_ui_start_restart_and_ipv6_loopback_isolation() -> TestResult {
    use std::{
        net::{Ipv4Addr, Ipv6Addr, SocketAddr},
        sync::Arc,
    };

    use ma2a_runtime::{
        Runtime,
        api::decode_command,
        current_user::CurrentUserRuntime,
        ipc::{IpcPaths, LocalApiClient, LocalApiServer},
        web::{WebAssets, WebAuthConfig, WebLifecycle, WebRuntimeDependencies},
    };
    use tokio_util::sync::CancellationToken;
    use zeroize::Zeroizing;

    let state = TempState::new("mapped-wildcard-lifecycle")?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let control = CurrentUserRuntime::open_at(
        state.path(),
        Arc::clone(&clock(6_000)) as Arc<dyn ma2a_runtime::web::Clock>,
        WebAuthConfig::default(),
    )
    .await?;
    control
        .web_auth()
        .change_password(
            ma2a_runtime::web::PasswordAction::Set,
            Zeroizing::new("web-binding-passphrase-9!".to_owned()),
        )
        .await?;
    let paths = IpcPaths::new(state.path())?;
    let client = LocalApiClient::new(paths.clone());
    let web = WebLifecycle::new(
        control.web_auth().clone(),
        WebRuntimeDependencies::new(
            control.web_auth().clone(),
            WebAssets::new(&[("index.html", b"ok")]),
            client.clone(),
        ),
    );
    let api_server = LocalApiServer::bind_for_launch(paths, runtime.handle(), control, None)
        .await?
        .with_web_lifecycle(web.clone());
    let cancellation = CancellationToken::new();
    let api_task = tokio::spawn(api_server.serve(cancellation.child_token()));

    let call = |payload: serde_json::Value| {
        let client = client.clone();
        async move {
            let raw = serde_json::to_vec(&payload)?;
            let bytes = client.call(&decode_command(&raw)?).await?;
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(serde_json::from_slice::<
                serde_json::Value,
            >(&bytes)?)
        }
    };
    let parse_url = |value: &serde_json::Value| -> Result<SocketAddr, Box<dyn std::error::Error + Send + Sync>> {
        let url = value
            .pointer("/result/payload/url")
            .and_then(serde_json::Value::as_str)
            .ok_or("missing ui_status url")?;
        Ok(url.trim_start_matches("http://").parse()?)
    };

    let first = parse_url(
        &call(serde_json::json!({"version": 1, "operation": "ui_start", "host": "::ffff:0.0.0.0", "port": 0})).await?,
    )?;
    let second = parse_url(
        &call(serde_json::json!({"version": 1, "operation": "ui_start", "host": "127.0.0.1", "port": first.port()})).await?,
    )?;
    assert_eq!(
        second,
        SocketAddr::from((Ipv4Addr::LOCALHOST, first.port()))
    );

    let mapped = parse_url(
        &call(serde_json::json!({"version": 1, "operation": "ui_start", "host": "::ffff:0.0.0.0", "port": 0})).await?,
    )?;
    let occupied_v6 = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, mapped.port()))?;
    let conflict = call(
        serde_json::json!({"version": 1, "operation": "ui_start", "host": "::1", "port": mapped.port()}),
    )
    .await?;
    assert_eq!(
        conflict.get("error").and_then(serde_json::Value::as_str),
        Some("unavailable")
    );
    let status =
        parse_url(&call(serde_json::json!({"version": 1, "operation": "ui_status"})).await?)?;
    assert_eq!(status, mapped);
    drop(tokio::net::TcpStream::connect((Ipv4Addr::LOCALHOST, mapped.port())).await?);

    drop(occupied_v6);
    let _ = web.stop().await;
    cancellation.cancel();
    api_task.await??;
    runtime.shutdown().await?;
    Ok(())
}
