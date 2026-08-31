use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use iroh::SecretKey;
use iroh::{Endpoint, RelayMode, Watcher as _, endpoint::presets};
use ma2a_core::{
    MemberCapabilities, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SpaceAuthorizationView, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{AdvertisementValidationContext, PrivateRelayAdvertisementValidator, RelayUrl};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation, StoreConfig};

pub(super) type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
pub(super) type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
pub(super) const NOW_MS: u64 = 1_700_000_000_000;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempState(PathBuf);

#[derive(Clone, Copy)]
pub(super) struct SpaceFixture<'a> {
    pub(super) local: &'a SecretKey,
    pub(super) relays: &'a [&'a SecretKey],
    pub(super) issued_at_ms: u64,
}

#[derive(Clone)]
pub(super) struct RelayFixture<'a> {
    pub(super) authorization: &'a SpaceAuthorizationView,
    pub(super) relay: &'a SecretKey,
    pub(super) relay_url: RelayUrl,
    pub(super) sequence: u64,
}

impl TempState {
    pub(super) fn new(name: &str) -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-phase-one-e2e-{name}-{}-{serial}",
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

    pub(super) fn config(&self) -> StoreConfig {
        StoreConfig::new(&self.0)
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn create_space(
    repository: &mut Repository,
    fixture: SpaceFixture<'_>,
) -> TestValue<SpaceAuthorizationView> {
    let local_member = SpaceMemberV1::new(
        fixture.local.public().into(),
        format!("local-{}", fixture.issued_at_ms),
        MemberCapabilities::new(true, false),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        fixture.issued_at_ms,
        local_member.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![local_member];
    for (index, relay) in fixture.relays.iter().enumerate() {
        members.push(SpaceMemberV1::new(
            relay.public().into(),
            format!("relay-{}-{index}", fixture.issued_at_ms),
            MemberCapabilities::new(true, true),
        )?);
    }
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        fixture.issued_at_ms + 1,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    let chain = repository
        .load_space_chain(created.space_id())?
        .ok_or("created Space chain is missing")?;
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

pub(super) fn store_relay(repository: &mut Repository, fixture: RelayFixture<'_>) -> TestResult {
    let signed = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(
            fixture.authorization.space_id(),
            fixture.relay.public().into(),
        ),
        fixture.relay_url,
        PrivateRelayAdvertisementValidity::new(fixture.sequence, NOW_MS, NOW_MS + 600_000)?,
    )?
    .sign(fixture.relay)?;
    PrivateRelayAdvertisementValidator::validate_and_store(
        repository,
        signed.canonical_bytes(),
        AdvertisementValidationContext::new(fixture.authorization, NOW_MS),
    )?;
    Ok(())
}

pub(super) fn emit(value: &serde_json::Value) {
    println!("{value}");
}

pub(super) async fn observed_home_endpoint(
    secret: &SecretKey,
    relay_map: iroh_relay::RelayMap,
    expected: &RelayUrl,
) -> TestValue<Endpoint> {
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(secret.clone())
        .relay_mode(RelayMode::Custom(relay_map))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    let mut homes = endpoint.home_relay_status();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if homes
                .get()
                .iter()
                .any(|status| status.url() == expected && status.is_connected())
            {
                break;
            }
            homes.updated().await?;
        }
        TestValue::Ok(())
    })
    .await??;
    Ok(endpoint)
}

pub(super) async fn wait_for_home_status(
    endpoint: &Endpoint,
    expected: &RelayUrl,
) -> TestValue<bool> {
    let mut homes = endpoint.home_relay_status();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if homes
                .get()
                .iter()
                .any(|status| status.url() == expected && status.is_connected())
            {
                return TestValue::Ok(true);
            }
            homes.updated().await?;
        }
    })
    .await?
}
