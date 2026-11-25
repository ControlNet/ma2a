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
use tokio::sync::broadcast::error::TryRecvError;

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
async fn failed_membership_observation_keeps_authoritative_state_unchanged() -> TestResult {
    // Given
    let state = TempState::new("membership-store-failure")?;
    let config = StoreConfig::new(state.path());
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let initial = handle.status().await?;
    let initial_metadata = Repository::open(&config)?.runtime_metadata()?;
    let mut events = handle.subscribe();
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch(
        "CREATE TRIGGER reject_endpoint_observation
         BEFORE UPDATE OF endpoint_observed_at_ms ON runtime_metadata
         BEGIN SELECT RAISE(ABORT, 'injected endpoint observation failure'); END;",
    )?;

    // When
    let error = handle
        .observe_memberships(vec![SpaceId::derive(b"uncommitted membership")])
        .await
        .expect_err("injected store failure unexpectedly succeeded");
    let after_failure = handle.status().await?;
    let after_failure_metadata = Repository::open(&config)?.runtime_metadata()?;

    // Then
    assert_eq!(error.code(), RuntimeErrorCode::STORE);
    assert_eq!(after_failure.membership_count(), initial.membership_count());
    assert_eq!(after_failure.revision(), initial.revision());
    assert_eq!(after_failure_metadata, initial_metadata);
    assert_eq!(events.try_recv(), Err(TryRecvError::Empty));
    connection.execute_batch("DROP TRIGGER reject_endpoint_observation;")?;
    runtime.shutdown().await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stored_relay_url_the_runtime_now_refuses_does_not_stop_startup() -> TestResult {
    // Given: an enabled public relay fallback whose URL is longer than the bound, written
    // before the bound existed.
    let state = TempState::new("overlong-relay")?;
    let config = StoreConfig::new(state.path());
    drop(Repository::open(&config)?);
    let overlong = format!("https://relay.invalid/{}", "a".repeat(512));
    let connection = Connection::open(config.database_path())?;
    connection.execute(
        "UPDATE relay_configuration SET public_fallback_enabled = 1, public_relay_url = ?1",
        [&overlong],
    )?;
    connection.execute(
        "INSERT INTO relay_public_fallback_urls(position, relay_url) VALUES (0, ?1)",
        [&overlong],
    )?;
    drop(connection);

    // When: the Runtime starts, twice.
    Runtime::start(config.clone()).await?.shutdown().await?;
    Runtime::start(config.clone()).await?.shutdown().await?;

    // Then: the fallback the Runtime cannot use is switched off rather than kept.
    let configuration = Repository::open(&config)?.relay_configuration()?;
    assert!(!configuration.public_fallback_enabled);
    assert!(configuration.public_relay_urls.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_usable_relay_spelled_longer_than_the_bound_survives_repair_of_another() -> TestResult {
    // Given: a private relay URL with credentials, which must be switched off, and a public
    // relay URL written longer than the bound whose parsed form is short.
    let state = TempState::new("long-spelled-relay")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.set_relay_configuration(&ma2a_store::RelayConfiguration {
        private_provider_enabled: true,
        listener_address: Some("127.0.0.1:3340".to_owned()),
        private_relay_url: Some("https://user:secret@relay.example.invalid/".to_owned()),
        served_spaces: vec![SpaceId::derive(b"long-spelled-relay")],
        transport: Some(ma2a_store::RelayTransportConfiguration::ExternalTlsTermination),
        ..repository.relay_configuration()?
    })?;
    drop(repository);
    let spelled = format!("https://relay.example.invalid/{}relay", "a/../".repeat(110));
    assert!(spelled.len() > ma2a_core::MAX_RELAY_URL_LEN);
    let connection = Connection::open(config.database_path())?;
    connection.execute(
        "UPDATE relay_configuration SET public_fallback_enabled = 1, public_relay_url = ?1",
        [&spelled],
    )?;
    connection.execute(
        "INSERT INTO relay_public_fallback_urls(position, relay_url) VALUES (0, ?1)",
        [&spelled],
    )?;
    drop(connection);

    // When: the Runtime starts, twice.
    Runtime::start(config.clone()).await?.shutdown().await?;
    Runtime::start(config.clone()).await?.shutdown().await?;

    // Then: the public relay keeps its meaning, in its parsed spelling, and the private relay
    // is switched off.
    let configuration = Repository::open(&config)?.relay_configuration()?;
    assert!(configuration.public_fallback_enabled);
    assert_eq!(
        configuration.public_relay_urls,
        vec!["https://relay.example.invalid/relay".to_owned()]
    );
    assert!(!configuration.private_provider_enabled);
    assert!(configuration.private_relay_url.is_none());
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
