//! Runtime Endpoint identity persistence and fail-closed integration tests.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_core::{EndpointId, SpaceId};
use ma2a_runtime::{Runtime, RuntimeErrorCode};
use ma2a_store::{KeyKind, KeyStore, Repository, StoreConfig};
use rusqlite::Connection;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new(name: &str) -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-runtime-{name}-{}-{serial}",
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

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_reuses_one_persisted_endpoint_identity() -> TestResult {
    // Given
    let state = TempState::new("restart")?;
    let config = StoreConfig::new(state.path());

    // When
    let first = Runtime::start(config.clone()).await?;
    let first_status = first.handle().status().await?;
    first.shutdown().await?;
    let second = Runtime::start(config.clone()).await?;
    let second_status = second.handle().status().await?;
    second.shutdown().await?;
    let third = Runtime::start(config).await?;
    let third_status = third.handle().status().await?;
    third.shutdown().await?;

    // Then
    assert_eq!(first_status.endpoint_id(), second_status.endpoint_id());
    assert_eq!(second_status.endpoint_id(), third_status.endpoint_id());
    assert!(first_status.is_ready());
    assert_eq!(first_status.membership_count(), 0);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn membership_observations_never_rotate_or_rebuild_identity() -> TestResult {
    // Given
    let state = TempState::new("memberships")?;
    let runtime = Runtime::start(StoreConfig::new(state.path())).await?;
    let handle = runtime.handle();
    let initial = handle.status().await?;
    let first_space = SpaceId::derive(b"first synthetic membership observation");
    let second_space = SpaceId::derive(b"second synthetic membership observation");

    // When
    handle
        .observe_memberships(vec![first_space, second_space])
        .await?;
    let joined = handle.status().await?;
    handle.observe_memberships(vec![second_space]).await?;
    let removed = handle.status().await?;
    runtime.shutdown().await?;

    // Then
    assert_eq!(joined.membership_count(), 2);
    assert_eq!(removed.membership_count(), 1);
    assert_eq!(initial.endpoint_id(), joined.endpoint_id());
    assert_eq!(joined.endpoint_id(), removed.endpoint_id());
    assert!(removed.revision() > initial.revision());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_endpoint_key_fails_closed_without_replacement() -> TestResult {
    // Given
    let state = TempState::new("corrupt-key")?;
    let config = StoreConfig::new(state.path());
    let runtime = Runtime::start(config.clone()).await?;
    runtime.shutdown().await?;
    let record = Repository::open(&config)?
        .endpoint()?
        .ok_or("endpoint record missing after startup")?;
    let key_store = KeyStore::open(state.path())?;
    let key_path = key_store.path_for(KeyKind::Endpoint, record.key_reference());
    fs::write(&key_path, [0xA5; 31])?;

    // When
    let error = start_error(config).await?;

    // Then
    assert_eq!(error, RuntimeErrorCode::INVALID_ENDPOINT_KEY);
    assert_eq!(fs::metadata(key_path)?.len(), 31);
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insecure_endpoint_key_permissions_fail_closed() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    // Given
    let state = TempState::new("key-permissions")?;
    let config = StoreConfig::new(state.path());
    let runtime = Runtime::start(config.clone()).await?;
    runtime.shutdown().await?;
    let record = Repository::open(&config)?
        .endpoint()?
        .ok_or("endpoint record missing after startup")?;
    let key_store = KeyStore::open(state.path())?;
    let key_path = key_store.path_for(KeyKind::Endpoint, record.key_reference());
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o644))?;

    // When
    let error = start_error(config).await?;

    // Then
    assert_eq!(error, RuntimeErrorCode::STORE);
    assert_eq!(fs::metadata(key_path)?.permissions().mode() & 0o777, 0o644);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stored_public_private_identity_mismatch_is_rejected() -> TestResult {
    // Given
    let state = TempState::new("identity-mismatch")?;
    let config = StoreConfig::new(state.path());
    let runtime = Runtime::start(config.clone()).await?;
    let original = runtime.handle().status().await?.endpoint_id();
    runtime.shutdown().await?;
    let replacement = different_valid_endpoint_id(original)?;
    let connection = Connection::open(config.database_path())?;
    connection.execute(
        "UPDATE endpoints SET endpoint_id = ?1 WHERE singleton = 1",
        [replacement.as_bytes().as_slice()],
    )?;

    // When
    let error = start_error(config).await?;

    // Then
    assert_eq!(error, RuntimeErrorCode::IDENTITY_MISMATCH);
    Ok(())
}

async fn start_error(config: StoreConfig) -> TestResultValue<RuntimeErrorCode> {
    match Runtime::start(config).await {
        Ok(runtime) => {
            runtime.shutdown().await?;
            Err("Runtime unexpectedly started".into())
        }
        Err(error) => Ok(error.code()),
    }
}

fn different_valid_endpoint_id(original: EndpointId) -> TestResultValue<EndpointId> {
    const PUBLIC_BYTES: [u8; 32] = [
        0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66,
    ];
    let candidate = EndpointId::try_from(PUBLIC_BYTES.as_slice())?;
    if candidate == original {
        Err("random Runtime identity matched fixed public test identity".into())
    } else {
        Ok(candidate)
    }
}
