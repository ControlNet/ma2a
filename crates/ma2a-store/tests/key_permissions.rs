//! Protected-key filesystem permission coverage.

#[path = "common/support.rs"]
mod support;

use ma2a_store::{
    EndpointRecord, KeyKind, KeyMaterial, KeyReference, KeyStore, Repository, StoreConfig,
    StoreError,
};
use support::{TempState, TestResult};

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
    assert_owner_only(&store.path_for(KeyKind::Endpoint, &endpoint_ref), 0o600)?;
    assert_owner_only(
        &store.path_for(KeyKind::SpaceAuthority, &authority_ref),
        0o600,
    )?;
    Ok(())
}

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
fn assert_owner_only(path: &std::path::Path, expected: u32) -> TestResult {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, expected);
    Ok(())
}

#[cfg(windows)]
fn assert_owner_only(path: &std::path::Path, _expected: u32) -> TestResult {
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$a=Get-Acl -LiteralPath $args[0]; if(!$a.AreAccessRulesProtected){exit 2}; $a.Access.IdentityReference.Value",
        ])
        .arg(path)
        .output()?;
    assert!(output.status.success());
    Ok(())
}
