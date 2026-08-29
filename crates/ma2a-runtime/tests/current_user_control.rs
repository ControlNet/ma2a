//! Typed current-user UI control integration coverage.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use ma2a_runtime::{
    api::{Command, UiControlResult},
    current_user::CurrentUserRuntime,
    web::{AuthFailure, Clock, WebAuthConfig},
};
use zeroize::Zeroizing;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-current-user-{}-{serial}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _result = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct ManualClock(AtomicI64);

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn password() -> Zeroizing<String> {
    Zeroizing::new("typed-control-passphrase-9!".to_owned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_control_changes_password_and_revokes_live_sessions() -> TestResult {
    // Given
    let state = TempState::new()?;
    let runtime = CurrentUserRuntime::open_at(
        state.path(),
        Arc::new(ManualClock(AtomicI64::new(8_000))),
        WebAuthConfig::default(),
    )
    .await?;

    // When
    let set = runtime.send(Command::ui_password_set(password())?).await?;
    let session = runtime.web_auth().login(password()).await?;
    let revoked = runtime.send(Command::session_revoke_all()?).await?;
    let old_session = runtime.web_auth().authenticate(session.bearer()).await;

    // Then
    assert!(matches!(
        set.ui_control_result(),
        Some(UiControlResult::PasswordSet(_))
    ));
    assert!(matches!(
        revoked.ui_control_result(),
        Some(UiControlResult::SessionsRevoked(_))
    ));
    assert!(matches!(old_session, Err(AuthFailure::Unauthorized)));
    Ok(())
}
