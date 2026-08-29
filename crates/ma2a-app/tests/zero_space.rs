//! Zero-Space Runtime Endpoint negotiation and shutdown integration test.

use std::{
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use iroh::{Endpoint, RelayMode, address_lookup::memory::MemoryLookup, endpoint::presets};
use ma2a_net::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS};
use ma2a_runtime::Runtime;
use ma2a_store::{Repository, StoreConfig};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-zero-space-{}-{serial}", std::process::id()));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_space_runtime_is_ready_rejects_normal_alpns_and_joins_shutdown() -> TestResult {
    // Given
    let state = TempState::new()?;
    let config = StoreConfig::new(state.path());
    let runtime = Runtime::start(config.clone()).await?;
    let status = runtime.handle().status().await?;
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_test_private"))
        .bind()
        .await?;

    // When
    for alpn in NORMAL_PROTOCOL_ALPNS {
        let attempt = tokio::time::timeout(
            Duration::from_secs(5),
            client.connect(status.endpoint_addr(), alpn),
        )
        .await?;
        assert!(attempt.is_err(), "normal ALPN unexpectedly negotiated");
    }
    let enrollment = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect(status.endpoint_addr(), ENROLLMENT_ALPN),
    )
    .await??;
    enrollment.close(0_u8.into(), b"");
    let _close_reason = enrollment.closed().await;
    client.close().await;
    let shutdown = runtime.shutdown().await?;

    // Then
    assert!(status.is_ready());
    assert_eq!(status.membership_count(), 0);
    assert!(status.connectivity().is_direct_only());
    assert_eq!(shutdown.joined_tasks(), 2);
    assert!(shutdown.endpoint_closed());
    let metadata = Repository::open(&config)?.runtime_metadata()?;
    assert_eq!(metadata.last_shutdown_clean(), Some(true));
    assert_eq!(metadata.boot_id(), Some(status.boot_id()));
    assert!(metadata.revision() >= status.revision());
    Ok(())
}
