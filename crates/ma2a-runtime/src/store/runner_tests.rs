//! The store thread's stop against requests that were already on their way in.

use std::{path::PathBuf, time::Duration};

use ma2a_store::StoreConfig;
use tokio::sync::{mpsc, oneshot};

use super::{STORE_CAPACITY, StoreBackend, StoreClient, StoreCommand};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TempState(PathBuf);

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_that_won_queue_room_before_stop_is_refused_not_stranded() -> TestResult {
    // Given: a running store and a caller that has room in the queue but has not
    // handed its request over yet, as `Sender::send` does between its two steps.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let state = TempState(std::env::temp_dir().join(format!(
        "ma2a-store-stop-{:016x}",
        u64::from_le_bytes(nonce)
    )));
    std::fs::create_dir(&state.0)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&state.0, std::fs::Permissions::from_mode(0o700))?;
    }
    let backend = StoreBackend::open(&StoreConfig::new(&state.0))?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let mut thread = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender.clone());
    let _identity = store.initialize().await?;
    let permit = sender.reserve().await?;

    // When: the Runtime stops the store, and only then the request lands.
    store.stop().await?;
    let _exited = tokio::time::timeout(Duration::from_millis(500), &mut thread).await;
    let (reply, response) = oneshot::channel();
    permit.send(StoreCommand::Revision(reply));

    // Then: the caller learns the store is gone instead of waiting forever.
    let answered = tokio::time::timeout(Duration::from_secs(5), response).await;
    assert!(
        matches!(answered, Ok(Err(_))),
        "a request that reached a stopped store must be refused, not left unanswered"
    );
    tokio::time::timeout(Duration::from_secs(5), thread).await??;
    Ok(())
}
