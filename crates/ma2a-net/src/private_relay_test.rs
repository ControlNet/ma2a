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

fn test_authorization(
    member: &SecretKey,
    marker: u8,
) -> Result<ma2a_core::SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let authority = ma2a_core::SpaceAuthoritySecret::from_bytes([marker; 32]);
    let member = ma2a_core::SpaceMemberV1::new(
        member.public().into(),
        format!("member-{marker:02x}"),
        ma2a_core::MemberCapabilities::new(true, false),
    )?;
    let genesis = ma2a_core::SpaceGenesisV1::new(
        ma2a_core::SpaceGenesisIdentity::new(
            [marker.wrapping_add(1); 32],
            1,
            authority.public_key(),
        )?,
        ma2a_core::SpaceGenesisOwner::new(member, ma2a_core::SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    Ok(ma2a_core::SpaceAuthorizationView::from_chain(
        &ma2a_core::SpaceChain::from_genesis(genesis)?,
    ))
}

#[tokio::test]
async fn revoked_connection_remains_tracked_until_on_disconnect() -> TestResult {
    use iroh_relay::server::{Access, AccessControl as _, ClientRequest};

    let member = SecretKey::from_bytes(&[0x74; 32]);
    let authorization = test_authorization(&member, 0x81)?;
    let access = PrivateRelayAccess::new(member.public().into(), &[authorization.space_id()]);
    access.replace_from_spaces(&[authorization]);
    let (parts, ()) = http::Request::builder()
        .uri("http://localhost/relay")
        .body(())?
        .into_parts();
    let request = ClientRequest::new(
        member.public(),
        iroh_relay::http::ProtocolVersion::V2,
        parts,
    );
    assert_eq!(access.on_connect(&request).await, Access::Allow);

    let first = access.replace_from_spaces(&[]);
    let second = access.replace_from_spaces(&[]);
    assert_eq!(first, vec![(member.public(), request.connection_id())]);
    assert_eq!(second, vec![(member.public(), request.connection_id())]);

    access.on_disconnect(member.public(), request.connection_id());
    assert!(access.replace_from_spaces(&[]).is_empty());
    Ok(())
}

#[tokio::test]
async fn member_revoked_while_relay_handshake_is_in_flight_is_disconnected() -> TestResult {
    use std::time::Duration;

    use iroh_dns::dns::DnsResolver;
    use iroh_relay::{
        client::ClientBuilder,
        tls::{CaTlsConfig, default_provider},
    };
    use n0_future::StreamExt as _;

    let member = SecretKey::from_bytes(&[0x75; 32]);
    let authorization = test_authorization(&member, 0x82)?;
    let served_space = authorization.space_id();
    let access = PrivateRelayAccess::new(member.public().into(), &[served_space]);
    access.replace_from_spaces(&[authorization]);
    let (entered_rx, release_tx) = access.arm_on_connect_gate();
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://relay.example.invalid".parse()?,
        ),
        vec![served_space],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let server = PrivateRelayServer::spawn(&config, access).await?;
    let relay_url: RelayUrl = format!("http://{}", server.listen_addr()).parse()?;
    let tls = CaTlsConfig::default().client_config(default_provider())?;

    let connecting = tokio::spawn(async move {
        ClientBuilder::new(relay_url, member, DnsResolver::new())
            .tls_client_config(tls)
            .connect()
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;

    // Revoke after on_connect recorded the connection and before Clients::register runs.
    server.replace_from_spaces(&[]);
    let _released = release_tx.send(());

    if let Ok(mut client) = tokio::time::timeout(Duration::from_secs(5), connecting).await?? {
        let closed = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .map_err(|_| "revoked client remained connected after in-flight handshake completed")?;
        assert!(closed.is_none() || closed.is_some_and(|message| message.is_err()));
    }
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn readmitting_member_during_in_flight_handshake_cancels_pending_disconnect() -> TestResult {
    use std::time::Duration;

    use iroh_dns::dns::DnsResolver;
    use iroh_relay::{
        client::ClientBuilder,
        tls::{CaTlsConfig, default_provider},
    };
    use n0_future::StreamExt as _;

    let member = SecretKey::from_bytes(&[0x76; 32]);
    let authorization = test_authorization(&member, 0x83)?;
    let served_space = authorization.space_id();
    let access = PrivateRelayAccess::new(member.public().into(), &[served_space]);
    access.replace_from_spaces(std::slice::from_ref(&authorization));
    let (entered_rx, release_tx) = access.arm_on_connect_gate();
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://relay.example.invalid".parse()?,
        ),
        vec![served_space],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let server = PrivateRelayServer::spawn(&config, access).await?;
    let relay_url: RelayUrl = format!("http://{}", server.listen_addr()).parse()?;
    let tls = CaTlsConfig::default().client_config(default_provider())?;

    let connecting = tokio::spawn(async move {
        ClientBuilder::new(relay_url, member, DnsResolver::new())
            .tls_client_config(tls)
            .connect()
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered_rx).await??;

    // Revoke then re-admit before registration completes.
    server.replace_from_spaces(&[]);
    server.replace_from_spaces(&[authorization]);
    let _released = release_tx.send(());

    let mut client = tokio::time::timeout(Duration::from_secs(5), connecting).await???;
    let stay_open = tokio::time::timeout(Duration::from_millis(150), client.next()).await;
    assert!(
        stay_open.is_err(),
        "re-admitted client should stay connected"
    );
    drop(client);
    server.shutdown().await?;
    Ok(())
}
