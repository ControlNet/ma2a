use std::sync::Arc;

use ma2a_store::StoreConfig;
use zeroize::Zeroizing;

use super::*;
use crate::web::{PasswordAction, SystemClock, WebAuthConfig, WebAuthService};

#[tokio::test]
async fn revocation_during_snapshot_read_prevents_notification()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = std::env::temp_dir().join(format!("ma2a-sse-revoke-poll-{}", std::process::id()));
    let state = TestState(path);
    let auth = WebAuthService::open(
        StoreConfig::new(&state.0),
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    auth.change_password(
        PasswordAction::Set,
        Zeroizing::new("test-only-sse-revocation-8!".to_owned()),
    )
    .await?;
    let session = auth
        .login(Zeroizing::new("test-only-sse-revocation-8!".to_owned()))
        .await?;

    // This deterministic projection future represents revocation committing while
    // the producer awaits IPC. Authentication itself uses the real SQLite Store.
    let result = authenticated_stamp(&auth, session.bearer(), async {
        auth.revoke_all_sessions()
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(SnapshotStamp {
            revision: 7,
            boot_id: "00112233445566778899aabbccddeeff".to_owned(),
        })
    })
    .await;
    assert_eq!(result, Err(StatusCode::UNAUTHORIZED));
    Ok(())
}

struct TestState(std::path::PathBuf);

impl Drop for TestState {
    fn drop(&mut self) {
        let _result = std::fs::remove_dir_all(&self.0);
    }
}
