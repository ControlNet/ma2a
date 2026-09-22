//! Process-level daemon-owned UI workflow coverage.

use std::sync::Arc;

use ma2a_runtime::{
    api::Command as ApiCommand,
    current_user::CurrentUserRuntime,
    web::{Clock, WebAuthConfig},
};
use zeroize::Zeroizing;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult};

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        1_800_000_000_000
    }
}

#[cfg(unix)]
#[tokio::test]
async fn ui_start_requires_password_and_returns_the_daemon_loopback_url() -> TestResult {
    // Given: a real detached daemon, because this test is about `restart`.
    let mut fixture = DaemonFixture::idle("cli-ui-workflow")?;
    let started = fixture.start_detached()?;
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    assert!(!fixture.run(&["ui", "start", "--json"])?.status.success());
    let control = CurrentUserRuntime::open_at(
        fixture.state_dir(),
        Arc::new(FixedClock),
        WebAuthConfig::default(),
    )
    .await?;
    control
        .send(ApiCommand::ui_password_set(Zeroizing::new(
            "secure-process-test-password".to_owned(),
        ))?)
        .await?;
    // When
    let output = fixture.run(&["ui", "start", "--json"])?;

    // Then
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let url = status
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or("missing UI URL")?;
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(!url.contains('@'));
    let restarted = fixture.run(&["restart"])?;
    assert!(
        restarted.status.success(),
        "{}",
        String::from_utf8_lossy(&restarted.stderr)
    );
    let stopped = fixture.run(&["ui", "status", "--json"])?;
    assert!(stopped.status.success());
    let stopped: serde_json::Value = serde_json::from_slice(&stopped.stdout)?;
    assert_eq!(
        stopped.get("running").and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert!(stopped.get("url").is_some_and(serde_json::Value::is_null));
    assert!(fixture.run(&["ui", "start"])?.status.success());
    fixture.shutdown()
}
