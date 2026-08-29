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
