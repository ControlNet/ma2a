//! Protected-key filesystem permission coverage.

#[path = "common/support.rs"]
mod support;

use ma2a_store::{KeyKind, KeyMaterial, KeyReference, KeyStore, StoreError};
use support::{TempState, TestResult, TestResultValue};

const RACE_SECRET_BYTES: usize = 8 * 1024 * 1024;

type PublicationOutcome = (Vec<u8>, Result<(), StoreError>);

struct Publication {
    store: KeyStore,
    reference: KeyReference,
    barrier: std::sync::Arc<std::sync::Barrier>,
    secret: Vec<u8>,
}

#[test]
fn key_files_are_separate_atomic_owner_only_files_and_zeroizing_reads() -> TestResult {
    // Given
    let state = TempState::new("key-files")?;
    let store = KeyStore::open(state.path())?;
    let endpoint_ref = KeyReference::parse("endpoint-main")?;
    let authority_ref = KeyReference::parse("authority-main")?;

    // When
    store.write(KeyMaterial::new(
        KeyKind::Endpoint,
        &endpoint_ref,
        b"endpoint material",
    ))?;
    store.write(KeyMaterial::new(
        KeyKind::SpaceAuthority,
        &authority_ref,
        b"authority material",
    ))?;

    // Then
    assert_eq!(
        store.read(KeyKind::Endpoint, &endpoint_ref)?.as_ref(),
        b"endpoint material"
    );
    assert_eq!(
        store
            .read(KeyKind::SpaceAuthority, &authority_ref)?
            .as_ref(),
        b"authority material"
    );
    assert_ne!(
        store.path_for(KeyKind::Endpoint, &endpoint_ref),
        store.path_for(KeyKind::SpaceAuthority, &authority_ref)
    );
    assert!(KeyReference::parse("../escape").is_err());
    assert!(KeyReference::parse("contains/slash").is_err());
    // On Windows each read above has already checked the key file's ACL.
    #[cfg(unix)]
    {
        assert_owner_only(&store.path_for(KeyKind::Endpoint, &endpoint_ref), 0o600)?;
        assert_owner_only(
            &store.path_for(KeyKind::SpaceAuthority, &authority_ref),
            0o600,
        )?;
    }
    Ok(())
}

#[test]
fn concurrent_writers_publish_exactly_one_immutable_key() -> TestResult {
    use std::sync::{Arc, Barrier};

    // Given
    let state = TempState::new("key-publication-race")?;
    let store = KeyStore::open(state.path())?;
    let reference = KeyReference::parse("endpoint-race")?;
    let barrier = Arc::new(Barrier::new(3));
    let first = publish_on_thread(Publication {
        store: store.clone(),
        reference: reference.clone(),
        barrier: Arc::clone(&barrier),
        secret: vec![0x11; RACE_SECRET_BYTES],
    });
    let second = publish_on_thread(Publication {
        store: store.clone(),
        reference: reference.clone(),
        barrier: Arc::clone(&barrier),
        secret: vec![0x22; RACE_SECRET_BYTES],
    });

    // When
    barrier.wait();
    let outcomes = [join_publication(first)?, join_publication(second)?];

    // Then
    assert_eq!(
        outcomes.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, result)| matches!(result, Err(StoreError::ProtectedKeyAlreadyExists)))
            .count(),
        1
    );
    let winner = outcomes
        .iter()
        .find_map(|(secret, result)| result.is_ok().then_some(secret.as_slice()))
        .ok_or("concurrent key publication had no winner")?;
    assert_eq!(store.read(KeyKind::Endpoint, &reference)?.as_ref(), winner);
    Ok(())
}

fn publish_on_thread(publication: Publication) -> std::thread::JoinHandle<PublicationOutcome> {
    std::thread::spawn(move || {
        publication.barrier.wait();
        let result = publication.store.write(KeyMaterial::new(
            KeyKind::Endpoint,
            &publication.reference,
            &publication.secret,
        ));
        (publication.secret, result)
    })
}

fn join_publication(
    handle: std::thread::JoinHandle<PublicationOutcome>,
) -> TestResultValue<PublicationOutcome> {
    handle
        .join()
        .map_err(|_| "key publication thread panicked".into())
}

#[cfg(unix)]
fn endpoint_id() -> Result<ma2a_core::EndpointId, ma2a_core::ProtocolError> {
    const BYTES: [u8; 32] = [
        0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
        0x66, 0x66,
    ];
    ma2a_core::EndpointId::try_from(BYTES.as_slice())
}

#[cfg(unix)]
#[test]
fn startup_rejects_loosened_key_permissions() -> TestResult {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use ma2a_store::{EndpointRecord, Repository, StoreConfig};

    // Given
    let state = TempState::new("key-startup-gate")?;
    let config = StoreConfig::new(state.path());
    let key_store = KeyStore::open(state.path())?;
    let reference = KeyReference::parse("endpoint-main")?;
    key_store.write(KeyMaterial::new(
        KeyKind::Endpoint,
        &reference,
        b"protected material",
    ))?;
    let mut repository = Repository::open(&config)?;
    repository.set_endpoint(&EndpointRecord::new(endpoint_id()?, reference.clone()))?;
    drop(repository);
    let key_path = key_store.path_for(KeyKind::Endpoint, &reference);
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o644))?;

    // When
    let result = Repository::open(&config);

    // Then
    assert!(matches!(
        result,
        Err(StoreError::InsecurePermissions { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn startup_refuses_a_dangling_symlink_database_without_touching_its_target() -> TestResult {
    use ma2a_store::{Repository, StoreConfig};

    // Given: the database name points at a file that does not exist yet, outside the state.
    let state = TempState::new("database-symlink")?;
    let outside = TempState::new("database-symlink-target")?;
    let target = outside.path().join("victim.sqlite3");
    let config = StoreConfig::new(state.path());
    std::os::unix::fs::symlink(&target, config.database_path())?;

    // When
    let result = Repository::open(&config);

    // Then
    assert!(
        matches!(result, Err(StoreError::InsecurePermissions { .. })),
        "{:?}",
        result.err()
    );
    assert!(!target.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn protecting_a_directory_never_changes_what_a_symlink_points_at() -> TestResult {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    // Given: the name to protect is a symlink to a directory with other permissions.
    let state = TempState::new("directory-symlink")?;
    let target = state.path().join("shared");
    fs::create_dir(&target)?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
    let link = state.path().join("link");
    std::os::unix::fs::symlink(&target, &link)?;

    // When
    let result = ma2a_store::protect_private_directory(&link, "test directory");

    // Then
    assert!(matches!(
        result,
        Err(StoreError::InsecurePermissions { .. })
    ));
    assert_eq!(fs::metadata(&target)?.permissions().mode() & 0o777, 0o755);
    Ok(())
}

#[cfg(unix)]
fn assert_owner_only(path: &std::path::Path, expected: u32) -> TestResult {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, expected);
    Ok(())
}

/// A verbatim state path such as `\\?\C:\…` is protected and accepted, the one spelling that can
/// name a directory ending in a dot included.
#[cfg(windows)]
#[test]
fn a_verbatim_state_path_is_protected_and_accepted() -> TestResult {
    // Given
    let state = TempState::new("verbatim")?;
    let verbatim = std::fs::canonicalize(state.path())?;
    assert!(verbatim.as_os_str().to_string_lossy().starts_with(r"\\?\"));
    let reference = KeyReference::parse("endpoint-main")?;

    for name in ["state", "state."] {
        // When
        let store = KeyStore::open(&verbatim.join(name))?;
        store.write(KeyMaterial::new(KeyKind::Endpoint, &reference, b"material"))?;

        // Then
        assert_eq!(
            store.read(KeyKind::Endpoint, &reference)?.as_ref(),
            b"material"
        );
    }
    Ok(())
}
