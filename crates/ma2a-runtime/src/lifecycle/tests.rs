use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_store::{Repository, StoreConfig};

use super::{Runtime, TaskExit};

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> Result<Self, Box<dyn Error + Send + Sync>> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-runtime-cancel-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_reports_the_persisted_observation_revision()
-> Result<(), Box<dyn Error + Send + Sync>> {
    // Given
    let state = TempState::new()?;
    let config = StoreConfig::new(&state.0);
    let Runtime {
        handle,
        cancellation,
        mut tasks,
        connections: _,
    } = Runtime::start(config.clone()).await?;

    // When
    cancellation.cancel();
    drop(handle);
    let mut actor_revision = None;
    while let Some(joined) = tasks.join_next().await {
        if let TaskExit::Actor(ack) = joined?? {
            actor_revision = Some(ack.revision);
        }
    }
    let persisted_revision = Repository::open(&config)?.runtime_metadata()?.revision();

    // Then
    assert_eq!(actor_revision, Some(persisted_revision));
    Ok(())
}

#[tokio::test]
async fn ready_event_is_observable_immediately_after_start()
-> Result<(), Box<dyn Error + Send + Sync>> {
    // Given
    let state = TempState::new()?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let mut events = runtime.handle().subscribe();

    // When
    tokio::task::yield_now().await;
    let event = events.try_recv()?;

    // Then
    assert!(event.is_ready());
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn failed_boot_commit_releases_bound_endpoint_before_returning()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let directory = TempState::new()?;
    let config = StoreConfig::new(&directory.0);
    let initial = Runtime::start(config.clone()).await?;
    let id = initial.handle().status().await?.endpoint_id();
    initial.shutdown().await?;
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch("CREATE TRIGGER fail_boot_commit BEFORE UPDATE ON runtime_metadata BEGIN SELECT RAISE(ABORT, 'test boot commit failure'); END;")?;
    assert!(Runtime::start(config.clone()).await.is_err());
    sql.execute_batch("DROP TRIGGER fail_boot_commit;")?;
    let restarted = Runtime::start(config).await?;
    assert_eq!(restarted.handle().status().await?.endpoint_id(), id);
    restarted.shutdown().await?;
    Ok(())
}
