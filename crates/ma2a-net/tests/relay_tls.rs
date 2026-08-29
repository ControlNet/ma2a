//! Private relay native and externally terminated TLS deployment coverage.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use iroh_base::{RelayUrl, SecretKey};
use ma2a_net::{
    NativeRelayTlsConfig, PrivateRelayAccess, PrivateRelayProviderConfig,
    PrivateRelayProviderLocation, PrivateRelayServer, PrivateRelayTransport, RelayTlsError,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TempState {
    path: PathBuf,
}

impl TempState {
    fn new(name: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-relay-tls-{name}-{}-{serial}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

fn write_certificate_fixture(
    state: &TempState,
    name: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf), Box<dyn std::error::Error + Send + Sync>> {
    let generated =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])?;
    let cert_path = state.path().join(format!("{name}.cert.pem"));
    let key_path = state.path().join(format!("{name}.key.pem"));
    fs::write(&cert_path, generated.cert.pem())?;
    fs::write(&key_path, generated.signing_key.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
    }
    Ok((cert_path, key_path))
}

#[tokio::test]
async fn native_tls_and_external_termination_servers_start_on_ephemeral_ports() -> TestResult {
    // Given
    let state = TempState::new("relay-tls-start")?;
    let (cert_path, key_path) = write_certificate_fixture(&state, "native")?;
    let native_space = ma2a_core::SpaceId::derive(b"native-relay-space");
    let native = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://localhost".parse::<RelayUrl>()?,
        ),
        vec![native_space],
        PrivateRelayTransport::NativeTls(NativeRelayTlsConfig::new(cert_path, key_path)),
    )?;
    let external_space = ma2a_core::SpaceId::derive(b"external-relay-space");
    let external = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://relay.example.invalid".parse::<RelayUrl>()?,
        ),
        vec![external_space],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;

    // When
    let native_access = PrivateRelayAccess::new(
        SecretKey::from_bytes(&[0x71; 32]).public().into(),
        &[native_space],
    );
    let external_access = PrivateRelayAccess::new(
        SecretKey::from_bytes(&[0x72; 32]).public().into(),
        &[external_space],
    );
    let native_server = PrivateRelayServer::spawn(&native, native_access).await?;
    let external_server = PrivateRelayServer::spawn(&external, external_access).await?;

    // Then
    assert!(native_server.listen_addr().port() > 0);
    assert!(native_server.uses_native_tls());
    assert!(external_server.listen_addr().port() > 0);
    assert!(!external_server.uses_native_tls());
    native_server.shutdown().await?;
    external_server.shutdown().await?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinked_tls_material_fails_closed() -> TestResult {
    // Given
    use std::os::unix::fs::symlink;

    let state = TempState::new("relay-tls-symlink")?;
    let (cert_path, key_path) = write_certificate_fixture(&state, "source")?;
    let linked_cert = state.path().join("linked.cert.pem");
    let linked_key = state.path().join("linked.key.pem");
    symlink(&cert_path, &linked_cert)?;
    symlink(&key_path, &linked_key)?;

    // When
    let certificate_result = NativeRelayTlsConfig::new(linked_cert, key_path).load_server_config();
    let key_result = NativeRelayTlsConfig::new(cert_path, linked_key).load_server_config();

    // Then
    assert!(matches!(
        certificate_result,
        Err(RelayTlsError::CertificateFile)
    ));
    assert!(matches!(key_result, Err(RelayTlsError::PrivateKeyFile)));
    Ok(())
}

#[test]
fn oversized_tls_material_fails_before_parsing() -> TestResult {
    // Given
    let state = TempState::new("relay-tls-oversized")?;
    let (cert_path, key_path) = write_certificate_fixture(&state, "bounded")?;
    let oversized_cert = state.path().join("oversized.cert.pem");
    let oversized_key = state.path().join("oversized.key.pem");
    fs::write(&oversized_cert, vec![b'x'; 1_048_577])?;
    fs::write(&oversized_key, vec![b'x'; 262_145])?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&oversized_key, fs::Permissions::from_mode(0o600))?;
    }

    // When
    let certificate_result =
        NativeRelayTlsConfig::new(oversized_cert, key_path).load_server_config();
    let key_result = NativeRelayTlsConfig::new(cert_path, oversized_key).load_server_config();

    // Then
    assert!(matches!(
        certificate_result,
        Err(RelayTlsError::CertificateFile)
    ));
    assert!(matches!(key_result, Err(RelayTlsError::PrivateKeyFile)));
    Ok(())
}

#[test]
fn mismatched_and_insecure_key_material_fail_closed() -> TestResult {
    // Given
    let state = TempState::new("relay-tls-rejections")?;
    let (cert_path, _matching_key) = write_certificate_fixture(&state, "certificate")?;
    let (_other_cert, mismatched_key) = write_certificate_fixture(&state, "other")?;
    let mismatched = NativeRelayTlsConfig::new(cert_path, mismatched_key.clone());

    // When
    let mismatch_result = mismatched.load_server_config();
    #[cfg(unix)]
    let insecure_result = {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&mismatched_key, fs::Permissions::from_mode(0o640))?;
        NativeRelayTlsConfig::new(state.path().join("other.cert.pem"), mismatched_key)
            .load_server_config()
    };

    // Then
    assert!(mismatch_result.is_err());
    #[cfg(unix)]
    assert!(insecure_result.is_err());
    Ok(())
}

#[cfg(unix)]
#[test]
fn private_key_with_special_permission_bits_fails_closed() -> TestResult {
    // Given
    use std::os::unix::fs::PermissionsExt as _;

    let state = TempState::new("relay-tls-special-bits")?;
    let (cert_path, key_path) = write_certificate_fixture(&state, "special-bits")?;
    fs::set_permissions(&key_path, fs::Permissions::from_mode(0o1600))?;

    // When
    let result = NativeRelayTlsConfig::new(cert_path, key_path).load_server_config();

    // Then
    assert!(matches!(
        result,
        Err(RelayTlsError::InsecurePrivateKeyPermissions)
    ));
    Ok(())
}
