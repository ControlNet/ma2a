use std::{
    cell::Cell,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use super::{
    KeyKind, KeyMaterial, KeyReference, KeyStore, PublicationOperations, StoreError, permissions,
    sync_parent,
};

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TestState {
    path: PathBuf,
}

impl TestState {
    fn new() -> Result<Self, StoreError> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-key-cleanup-{}-{serial}", std::process::id()));
        permissions::ensure_private_dir(&path, "test state directory")?;
        Ok(Self { path })
    }
}

impl Drop for TestState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

struct CleanupDeniedOperations {
    published: Cell<bool>,
    parent_sync_reached: Cell<bool>,
    destination_validated: Cell<bool>,
}

impl CleanupDeniedOperations {
    const fn new() -> Self {
        Self {
            published: Cell::new(false),
            parent_sync_reached: Cell::new(false),
            destination_validated: Cell::new(false),
        }
    }
}

impl PublicationOperations for CleanupDeniedOperations {
    fn hard_link(&self, temporary: &Path, destination: &Path) -> io::Result<()> {
        fs::hard_link(temporary, destination)?;
        self.published.set(true);
        Ok(())
    }

    fn remove_temporary(&self, temporary: &Path) -> io::Result<()> {
        if self.published.get() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected post-publication cleanup denial",
            ));
        }
        fs::remove_file(temporary)
    }

    fn sync_parent(&self, destination: &Path) -> Result<(), StoreError> {
        self.parent_sync_reached.set(true);
        sync_parent(destination)
    }

    fn validate_destination(&self, destination: &Path) -> Result<(), StoreError> {
        permissions::validate_private_file(destination, "protected-key file")?;
        self.destination_validated.set(true);
        Ok(())
    }
}

#[test]
fn temporary_cleanup_failure_after_publication_is_non_fatal_and_retry_cannot_replace() -> TestResult
{
    // Given
    let state = TestState::new()?;
    let store = KeyStore::open(&state.path)?;
    let reference = KeyReference::parse("post-publication-cleanup")?;
    let original = [0x31; 32];
    let replacement = [0x52; 32];
    let operations = CleanupDeniedOperations::new();

    // When
    let first_result = store.write_with_operations(
        KeyMaterial::new(KeyKind::Endpoint, &reference, &original),
        &operations,
    );

    // Then
    assert!(first_result.is_ok());
    assert!(operations.parent_sync_reached.get());
    assert!(operations.destination_validated.get());
    let destination = store.path_for(KeyKind::Endpoint, &reference);
    assert_eq!(
        store.read(KeyKind::Endpoint, &reference)?.as_ref(),
        original
    );
    permissions::validate_private_file(&destination, "protected-key file")?;

    let aliases = fs::read_dir(
        destination
            .parent()
            .ok_or(StoreError::InvalidStateDirectory)?,
    )?
    .map(|entry| entry.map(|value| value.path()))
    .collect::<Result<Vec<_>, _>>()?;
    let temporary_aliases = aliases
        .iter()
        .filter(|path| **path != destination)
        .collect::<Vec<_>>();
    assert!(temporary_aliases.len() <= 1);
    for alias in temporary_aliases {
        permissions::validate_private_file(alias, "protected-key temporary file")?;
        assert_eq!(fs::read(alias)?, original);
    }

    let retry = store.write(KeyMaterial::new(
        KeyKind::Endpoint,
        &reference,
        &replacement,
    ));
    assert!(matches!(retry, Err(StoreError::ProtectedKeyAlreadyExists)));
    assert_eq!(
        store.read(KeyKind::Endpoint, &reference)?.as_ref(),
        original
    );
    Ok(())
}
