//! Private relay publication and persisted runtime configuration coverage.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use iroh_base::{RelayUrl, SecretKey};
use ma2a_core::{
    MemberCapabilities, SignedSpaceGenesisV1, SpaceAuthoritySecret, SpaceAuthorizationView,
    SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_net::{
    AdvertisementPublicationRequest, PrivateRelayAdvertisementPublisher,
    PrivateRelayProviderConfig, PrivateRelayProviderLocation, PrivateRelayTransport,
    PublicRelayFallbackConfig, RuntimeRelayConfiguration,
};
use ma2a_store::{RelayConfiguration, RelayTransportConfiguration, Repository, StoreConfig};

const NOW_MS: u64 = 1_700_000_000_000;
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct TempState {
    path: PathBuf,
}

impl TempState {
    fn new(name: &str) -> TestResult<Self> {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-relay-publication-{name}-{}-{serial}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
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

fn authorization(
    provider: &SecretKey,
    marker: u8,
    relay_capability: bool,
) -> Result<(SignedSpaceGenesisV1, SpaceAuthorizationView), Box<dyn std::error::Error + Send + Sync>>
{
    let authority = SpaceAuthoritySecret::from_bytes([marker; 32]);
    let member = SpaceMemberV1::new(
        provider.public().into(),
        format!("provider-{marker:02x}"),
        MemberCapabilities::new(true, relay_capability),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([marker.wrapping_add(1); 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let chain = SpaceChain::from_genesis(genesis.clone())?;
    Ok((genesis, SpaceAuthorizationView::from_chain(&chain)))
}

#[test]
fn publisher_emits_one_space_private_advertisement_per_served_space() -> TestResult {
    // Given
    let secret_bytes = [0x35; 32];
    let provider = SecretKey::from_bytes(&secret_bytes);
    let (_first_genesis, first_authorization) = authorization(&provider, 0x44, true)?;
    let (_second_genesis, second_authorization) = authorization(&provider, 0x45, true)?;
    let first_space = first_authorization.space_id();
    let second_space = second_authorization.space_id();
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            "127.0.0.1:0".parse()?,
            "https://relay.example.invalid".parse()?,
        ),
        vec![first_space, second_space],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let publisher = PrivateRelayAdvertisementPublisher::new(
        ma2a_net::EndpointSecret::parse(&secret_bytes)?,
        config,
    );
    let state = TempState::new("relay-publisher-sequence")?;
    let store_config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&store_config)?;

    // When
    let advertisements = publisher.advertisements(
        &mut repository,
        AdvertisementPublicationRequest::new(
            &[first_authorization.clone(), second_authorization.clone()],
            NOW_MS,
            NOW_MS + 600_000,
        ),
    )?;
    drop(repository);
    let mut repository = Repository::open(&store_config)?;
    let after_restart = publisher.advertisements(
        &mut repository,
        AdvertisementPublicationRequest::new(
            &[first_authorization, second_authorization],
            NOW_MS + 1,
            NOW_MS + 600_001,
        ),
    )?;

    // Then
    assert_eq!(advertisements.len(), 2);
    assert_eq!(
        advertisements
            .iter()
            .map(|signed| signed.advertisement().space_id())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([first_space, second_space])
    );
    assert!(advertisements.iter().all(|signed| {
        signed.advertisement().provider_endpoint_id() == provider.public().into()
            && signed.advertisement().sequence() == 1
    }));
    assert!(
        after_restart
            .iter()
            .all(|signed| signed.advertisement().sequence() == 2)
    );
    Ok(())
}

#[test]
fn publisher_includes_current_members_when_space_policy_allows_relay() -> TestResult {
    // Given
    let secret_bytes = [0x37; 32];
    let provider = SecretKey::from_bytes(&secret_bytes);
    let (_capable_genesis, capable) = authorization(&provider, 0x47, true)?;
    let (_incapable_genesis, incapable) = authorization(&provider, 0x48, false)?;
    let incapable_space_id = incapable.space_id();
    let config = PrivateRelayProviderConfig::new(
        PrivateRelayProviderLocation::new(
            "127.0.0.1:0".parse()?,
            "https://relay.example.invalid".parse()?,
        ),
        vec![capable.space_id(), incapable.space_id()],
        PrivateRelayTransport::ExternalTlsTermination,
    )?;
    let publisher = PrivateRelayAdvertisementPublisher::new(
        ma2a_net::EndpointSecret::parse(&secret_bytes)?,
        config,
    );
    let state = TempState::new("relay-publisher-effective-spaces")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;

    // When
    let advertisements = publisher.advertisements(
        &mut repository,
        AdvertisementPublicationRequest::new(
            &[capable.clone(), incapable],
            NOW_MS,
            NOW_MS + 600_000,
        ),
    )?;

    // Then
    assert_eq!(advertisements.len(), 2);
    let advertised_spaces = advertisements
        .iter()
        .map(|advertisement| advertisement.advertisement().space_id())
        .collect::<Vec<_>>();
    assert!(advertised_spaces.contains(&capable.space_id()));
    assert!(advertised_spaces.contains(&incapable_space_id));
    Ok(())
}

#[test]
fn public_fallback_is_transport_only_and_not_advertisement_data() -> TestResult {
    // Given
    let public_url: RelayUrl = "https://public-relay.example.invalid".parse()?;
    let fallback = PublicRelayFallbackConfig::new(vec![public_url.clone()])?;

    // When
    let relay_mode = fallback.relay_mode();
    let relay_map = relay_mode.relay_map();

    // Then
    assert_eq!(relay_map.len(), 1);
    assert!(relay_map.contains(&public_url));
    assert_eq!(fallback.relay_urls(), &[public_url]);
    Ok(())
}

#[test]
fn store_owned_relay_configuration_parses_into_runtime_types() -> TestResult {
    // Given
    let served_space = ma2a_core::SpaceId::derive(b"persisted-runtime-relay-space");
    let persisted = RelayConfiguration {
        public_fallback_enabled: true,
        public_relay_urls: vec![
            "https://public-a.example.invalid".to_owned(),
            "https://public-b.example.invalid".to_owned(),
        ],
        private_provider_enabled: true,
        listener_address: Some("127.0.0.1:8443".to_owned()),
        private_relay_url: Some("https://private.example.invalid".to_owned()),
        served_spaces: vec![served_space],
        transport: Some(RelayTransportConfiguration::ExternalTlsTermination),
    };

    // When
    let runtime = RuntimeRelayConfiguration::try_from(persisted)?;

    // Then
    assert_eq!(
        runtime
            .public_fallback()
            .map(|config| config.relay_urls().len()),
        Some(2)
    );
    assert_eq!(
        runtime
            .private_provider()
            .map(PrivateRelayProviderConfig::served_spaces),
        Some([served_space].as_slice())
    );
    Ok(())
}
