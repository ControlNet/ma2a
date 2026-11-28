//! A natively terminated private relay opens no plaintext listener beyond loopback.
use std::{fs, net::Ipv4Addr};

use iroh_base::{RelayUrl, SecretKey};

use crate::{
    NativeRelayTlsConfig, PrivateRelayAccess, PrivateRelayProviderConfig,
    PrivateRelayProviderLocation, PrivateRelayServer, PrivateRelayTransport,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn native_tls_on_every_interface_keeps_its_plain_listener_on_loopback() -> TestResult {
    // Given: a provider listening with native TLS on every IPv4 interface.
    let directory =
        std::env::temp_dir().join(format!("ma2a-private-relay-plain-{}", std::process::id()));
    fs::create_dir_all(&directory)?;
    let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])?;
    let (cert_path, key_path) = (directory.join("cert.pem"), directory.join("key.pem"));
    fs::write(&cert_path, generated.cert.pem())?;
    fs::write(&key_path, generated.signing_key.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600))?;
    }
    let space = ma2a_core::SpaceId::derive(b"private-relay-plain-listener");
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::UNSPECIFIED, 0).into(),
            "https://localhost".parse::<RelayUrl>()?,
        ),
        vec![space],
        PrivateRelayTransport::NativeTls(NativeRelayTlsConfig::new(cert_path, key_path)),
    )?;
    let access =
        PrivateRelayAccess::new(SecretKey::from_bytes(&[0x73; 32]).public().into(), &[space]);

    // When
    let server = PrivateRelayServer::spawn(&config, access).await?;

    // Then: the TLS listener takes every interface, the plain one only loopback.
    let plain = server.server.http_addr().ok_or("no plain listener")?;
    assert!(server.listen_addr().ip().is_unspecified());
    assert!(plain.ip().is_loopback(), "{plain}");
    server.shutdown().await?;
    let _cleanup = fs::remove_dir_all(&directory);
    Ok(())
}

#[test]
fn late_tls_material_errors_are_retryable_configuration_failures() {
    for error in [
        crate::RelayTlsError::CertificateFile,
        crate::RelayTlsError::PrivateKeyFile,
        crate::RelayTlsError::InsecurePrivateKeyPermissions,
        crate::RelayTlsError::InvalidCertificate,
        crate::RelayTlsError::CertificateNotCurrent,
        crate::RelayTlsError::InvalidPrivateKey,
        crate::RelayTlsError::KeyMismatch,
    ] {
        assert!(
            crate::PrivateRelayServerError::Tls(error).is_retryable_configuration(),
            "{error:?} should be retryable"
        );
    }
    assert!(
        !crate::PrivateRelayServerError::Tls(crate::RelayTlsError::TlsConfiguration)
            .is_retryable_configuration()
    );
}
