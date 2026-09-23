use std::{
    error::Error,
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use ma2a_store::{RelayConfiguration, RelayTransportConfiguration, Repository, StoreConfig};

use crate::{Runtime, RuntimeClock, error::RuntimeError};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
const NOW_MS: i64 = 1_700_000_000_000;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(NOW_MS)
    }
}

#[derive(Debug)]
struct AdvancingClock(AtomicI64);

impl RuntimeClock for AdvancingClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

pub(super) struct TempState(pub(super) PathBuf);

impl TempState {
    pub(super) fn new(label: &str) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-relay-lifecycle-{label}-{}-{serial}",
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
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configure_removed_space_and_disable_reconcile_active_advertisements() -> TestResult {
    // Given
    let state = TempState::new("configure")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let handle = runtime.handle();
    let first_space = handle.create_owned_space("first".to_owned()).await?;
    let second_space = handle.create_owned_space("second".to_owned()).await?;

    // When
    handle
        .set_relay_configuration(enabled_configuration(vec![first_space, second_space]))
        .await?;

    // Then
    let endpoint_id = handle.status().await?.endpoint_id();
    let repository = Repository::open(&config)?;
    let first = repository
        .relay_advertisement(first_space, endpoint_id)?
        .ok_or("first configured advertisement missing")?;
    let second = repository
        .relay_advertisement(second_space, endpoint_id)?
        .ok_or("second configured advertisement missing")?;
    assert!(first.is_active() && second.is_active());
    let first_sequence = first.sequence();
    let second_sequence = second.sequence();
    drop(repository);

    handle
        .set_relay_configuration(enabled_configuration(vec![first_space]))
        .await?;
    let repository = Repository::open(&config)?;
    let first = repository
        .relay_advertisement(first_space, endpoint_id)?
        .ok_or("retained advertisement missing")?;
    let second = repository
        .relay_advertisement(second_space, endpoint_id)?
        .ok_or("removed advertisement high-water missing")?;
    assert!(first.is_active());
    assert!(first.sequence() > first_sequence);
    assert!(!second.is_active());
    assert_eq!(second.sequence(), second_sequence);
    let second_control = repository
        .control_spaces_for(endpoint_id)?
        .into_iter()
        .find(|space| space.chain().space_id() == second_space)
        .ok_or("removed Space control state missing")?;
    assert_eq!(second_control.active_relay_advertisements().count(), 0);
    drop(repository);

    handle
        .set_relay_configuration(disabled_configuration())
        .await?;
    let repository = Repository::open(&config)?;
    assert!(
        !repository
            .relay_advertisement(first_space, endpoint_id)?
            .ok_or("disabled advertisement high-water missing")?
            .is_active()
    );
    let control_spaces = repository.control_spaces_for(endpoint_id)?;
    assert!(
        control_spaces
            .iter()
            .all(|space| space.active_relay_advertisements().next().is_none())
    );
    let relay_map = ma2a_net::LocalIrohRelayMap::from_control_spaces(
        &control_spaces,
        None,
        u64::try_from(NOW_MS)?,
    );
    assert!(relay_map.private_relays().next().is_none());
    drop(repository);
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_refreshes_persisted_private_relay_advertisement() -> TestResult {
    // Given
    let state = TempState::new("restart")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let handle = runtime.handle();
    let space_id = handle.create_owned_space("restart".to_owned()).await?;
    handle
        .set_relay_configuration(enabled_configuration(vec![space_id]))
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    let initial_sequence = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("initial restart advertisement missing")?
        .sequence();
    runtime.shutdown().await?;

    // When
    let restarted = Runtime::start_with_clock(config.clone(), Arc::new(FixedClock)).await?;
    let restarted_handle = restarted.handle();
    let _status = restarted_handle.status().await?;

    // Then
    let refreshed = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("restarted advertisement missing")?;
    assert!(refreshed.is_active());
    assert!(refreshed.sequence() > initial_sequence);
    restarted.shutdown().await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn periodic_actor_maintenance_refreshes_private_relay_advertisement() -> TestResult {
    // Given
    let state = TempState::new("periodic")?;
    let config = StoreConfig::new(&state.0);
    let clock = Arc::new(AdvancingClock(AtomicI64::new(NOW_MS)));
    let runtime =
        Runtime::start_with_clock(config.clone(), Arc::<AdvancingClock>::clone(&clock)).await?;
    let handle = runtime.handle();
    let space_id = handle.create_owned_space("periodic".to_owned()).await?;
    handle
        .set_relay_configuration(enabled_configuration(vec![space_id]))
        .await?;
    let endpoint_id = handle.status().await?.endpoint_id();
    let initial_sequence = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("initial periodic advertisement missing")?
        .sequence();
    let initial_revision = Repository::open(&config)?.revision()?;
    let initial_publication_rounds = handle
        .control_schedules()
        .iter()
        .filter(|trigger| {
            matches!(
                trigger,
                crate::control_sync::ControlRoundTrigger::AddressAdvanced
                    | crate::control_sync::ControlRoundTrigger::RelayAdvanced
            )
        })
        .count();

    // An unchanged maintenance turn must not reserve another signed sequence.
    tokio::time::advance(crate::control_actor::control_period(endpoint_id)).await;
    tokio::task::yield_now().await;
    let _status = handle.status().await?;
    let unchanged = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("unchanged periodic advertisement missing")?;
    assert_eq!(unchanged.sequence(), initial_sequence);
    assert_eq!(Repository::open(&config)?.revision()?, initial_revision);
    assert_eq!(
        handle
            .control_schedules()
            .iter()
            .filter(|trigger| {
                matches!(
                    trigger,
                    crate::control_sync::ControlRoundTrigger::AddressAdvanced
                        | crate::control_sync::ControlRoundTrigger::RelayAdvanced
                )
            })
            .count(),
        initial_publication_rounds
    );

    // Once the authoritative clock reaches the renewal window, refresh it.
    clock.0.store(NOW_MS + 300_000, Ordering::Relaxed);
    tokio::time::advance(crate::control_actor::control_period(endpoint_id)).await;
    tokio::task::yield_now().await;
    let _status = handle.status().await?;

    // Then
    let refreshed = Repository::open(&config)?
        .relay_advertisement(space_id, endpoint_id)?
        .ok_or("periodic advertisement missing")?;
    assert!(refreshed.sequence() > initial_sequence);
    runtime.shutdown().await?;
    Ok(())
}

pub(super) fn enabled_configuration(served_spaces: Vec<ma2a_core::SpaceId>) -> RelayConfiguration {
    RelayConfiguration {
        public_fallback_enabled: false,
        public_relay_urls: Vec::new(),
        private_provider_enabled: true,
        listener_address: Some("127.0.0.1:0".to_owned()),
        private_relay_url: Some("https://relay.example.invalid".to_owned()),
        served_spaces,
        transport: Some(RelayTransportConfiguration::ExternalTlsTermination),
    }
}

pub(super) fn disabled_configuration() -> RelayConfiguration {
    RelayConfiguration {
        public_fallback_enabled: false,
        public_relay_urls: Vec::new(),
        private_provider_enabled: false,
        listener_address: None,
        private_relay_url: None,
        served_spaces: Vec::new(),
        transport: None,
    }
}
