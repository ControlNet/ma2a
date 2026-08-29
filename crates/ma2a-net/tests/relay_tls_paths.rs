//! Real HTTPS client coverage for both private relay TLS deployment modes.

use std::{
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use iroh_base::{RelayUrl, SecretKey};
use iroh_dns::dns::DnsResolver;
use iroh_relay::{
    client::ClientBuilder,
    tls::{CaTlsConfig, default_provider},
};
use ma2a_core::{
    MemberCapabilities, SpaceAuthoritySecret, SpaceAuthorizationView, SpaceChain,
    SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    NativeRelayTlsConfig, PrivateRelayAccess, PrivateRelayProviderConfig,
    PrivateRelayProviderLocation, PrivateRelayServer, PrivateRelayTransport,
};
use n0_future::SinkExt as _;
use rustls_pki_types::{CertificateDer, pem::PemObject as _};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    task::JoinSet,
};
use tokio_rustls::TlsAcceptor;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct TempState(PathBuf);

impl TempState {
    fn new(name: &str) -> TestResult<Self> {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-relay-tls-path-{name}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
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

fn authorization(provider: &SecretKey, marker: u8) -> TestResult<SpaceAuthorizationView> {
    let authority = SpaceAuthoritySecret::from_bytes([marker; 32]);
    let member = SpaceMemberV1::new(
        provider.public().into(),
        format!("provider-{marker:02x}"),
        MemberCapabilities::new(true, true),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([marker.wrapping_add(1); 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    Ok(SpaceAuthorizationView::from_chain(
        &SpaceChain::from_genesis(genesis)?,
    ))
}

fn certificate_fixture(
    state: &TempState,
) -> TestResult<(PathBuf, PathBuf, Vec<CertificateDer<'static>>)> {
    let generated =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned(), "127.0.0.1".to_owned()])?;
    let certificate_path = state.path().join("relay.cert.pem");
    let private_key_path = state.path().join("relay.key.pem");
    let certificate_pem = generated.cert.pem();
    fs::write(&certificate_path, &certificate_pem)?;
    fs::write(&private_key_path, generated.signing_key.serialize_pem())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&private_key_path, fs::Permissions::from_mode(0o600))?;
    }
    let roots = CertificateDer::pem_slice_iter(certificate_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()?;
    Ok((certificate_path, private_key_path, roots))
}

#[tokio::test]
async fn native_tls_accepts_a_real_https_iroh_client() -> TestResult {
    // Given
    let state = TempState::new("native")?;
    let (certificate_path, private_key_path, roots) = certificate_fixture(&state)?;
    let provider = SecretKey::from_bytes(&[0x73; 32]);
    let authorization = authorization(&provider, 0x74)?;
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://localhost".parse()?,
        ),
        vec![authorization.space_id()],
        PrivateRelayTransport::NativeTls(NativeRelayTlsConfig::new(
            certificate_path,
            private_key_path,
        )),
    )?;
    let access = PrivateRelayAccess::new(provider.public().into(), config.served_spaces());
    access.replace_from_spaces(&[authorization]);
    let server = PrivateRelayServer::spawn(&config, access).await?;
    let relay_url: RelayUrl =
        format!("https://localhost:{}", server.listen_addr().port()).parse()?;
    let tls = CaTlsConfig::custom_roots(roots).client_config(default_provider())?;

    // When
    let mut client = ClientBuilder::new(relay_url, provider, DnsResolver::new())
        .tls_client_config(tls)
        .connect()
        .await?;

    // Then
    client.close().await?;
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn external_mode_accepts_https_through_a_real_tls_terminator() -> TestResult {
    // Given
    let state = TempState::new("external")?;
    let (certificate_path, private_key_path, roots) = certificate_fixture(&state)?;
    let provider = SecretKey::from_bytes(&[0x75; 32]);
    let authorization = authorization(&provider, 0x76)?;
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            (Ipv4Addr::LOCALHOST, 0).into(),
            "https://localhost".parse()?,
        ),
        vec![authorization.space_id()],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let access = PrivateRelayAccess::new(provider.public().into(), config.served_spaces());
    access.replace_from_spaces(&[authorization]);
    let server = PrivateRelayServer::spawn(&config, access).await?;
    let terminator_config =
        NativeRelayTlsConfig::new(certificate_path, private_key_path).load_server_config()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let terminator_addr = listener.local_addr()?;
    let backend_addr = server.listen_addr();
    let acceptor = TlsAcceptor::from(Arc::new(terminator_config));
    let mut terminator = JoinSet::new();
    terminator.spawn(async move {
        let (incoming, _) = listener.accept().await?;
        let mut frontend = acceptor.accept(incoming).await?;
        let mut backend = TcpStream::connect(backend_addr).await?;
        copy_bidirectional(&mut frontend, &mut backend).await?;
        TestResult::Ok(())
    });
    let relay_url: RelayUrl = format!("https://localhost:{}", terminator_addr.port()).parse()?;
    let tls = CaTlsConfig::custom_roots(roots).client_config(default_provider())?;

    // When
    let mut client = ClientBuilder::new(relay_url, provider, DnsResolver::new())
        .tls_client_config(tls)
        .connect()
        .await?;

    // Then
    client.close().await?;
    let joined = tokio::time::timeout(Duration::from_secs(5), terminator.join_next()).await?;
    joined.ok_or("TLS terminator task was missing")???;
    server.shutdown().await?;
    Ok(())
}
