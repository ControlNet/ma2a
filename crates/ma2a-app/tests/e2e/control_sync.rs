use std::{fmt::Write as _, time::Duration};

use iroh::SecretKey;
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity,
    PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity,
    SpaceAddressRecordV1,
};
use ma2a_runtime::Runtime;
use ma2a_store::{Repository, StoreConfig};

use super::harness::{TestResult, emit};
use crate::control_sync_e2e_support as support;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_dial_propagates_manifest_address_and_relay_without_existing_connection()
-> TestResult {
    // Given
    let fixture = support::control_fixture().await?;
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let candidate = Runtime::start_with_clock(candidate_config.clone(), support::clock()).await?;
    assert!(
        candidate
            .connections()
            .observations(fixture.owner_secret.public().into())
            .iter()
            .all(|observation| observation.last_success_at_ms().is_none())
    );
    let owner = Runtime::start_with_clock(owner_config.clone(), support::clock()).await?;

    // When
    tokio::time::timeout(Duration::from_secs(15), candidate.handle().sync_control()).await??;

    // Then
    let repository = Repository::open(&candidate_config)?;
    assert_eq!(fixture.owner_records.len(), fixture.shared_spaces.len());
    assert_eq!(fixture.candidate_records.len(), fixture.shared_spaces.len());
    assert_eq!(
        fixture.owner_advertisements.len(),
        fixture.shared_spaces.len()
    );
    let mut manifest_high_water = Vec::new();
    let mut address_high_water = Vec::new();
    let mut relay_high_water = Vec::new();
    for space_id in fixture.shared_spaces {
        manifest_high_water.push(
            repository
                .load_space_chain(space_id)?
                .ok_or("synchronized chain missing")?
                .latest_generation(),
        );
        address_high_water.push(
            repository
                .address_record(space_id, fixture.owner_secret.public().into())?
                .ok_or("synchronized address missing")?
                .sequence(),
        );
        relay_high_water.push(
            repository
                .relay_advertisement(space_id, fixture.owner_secret.public().into())?
                .ok_or("synchronized relay missing")?
                .sequence(),
        );
    }
    assert!(
        repository
            .load_space_chain(fixture.private_space)?
            .is_none()
    );
    emit(&serde_json::json!({
        "scenario": "control-sync-active-dial",
        "endpoint_ids": {
            "owner": fixture.owner_secret.public().to_string(),
            "candidate": fixture.candidate_secret.public().to_string()
        },
        "manifest_high_water": manifest_high_water,
        "address_high_water": address_high_water,
        "relay_high_water": relay_high_water,
        "private_space_leaked": false
    }));
    candidate.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_preserves_endpoint_identity_and_control_high_water() -> TestResult {
    // Given
    let fixture = support::control_fixture().await?;
    let config: StoreConfig = fixture.candidate_config();
    let owner = Runtime::start_with_clock(fixture.owner_config(), support::clock()).await?;
    let first = Runtime::start_with_clock(config.clone(), support::clock()).await?;
    let first_id = first.handle().status().await?.endpoint_id();
    tokio::time::timeout(Duration::from_secs(15), first.handle().sync_control()).await??;
    first.shutdown().await?;
    owner.shutdown().await?;
    let before = persisted_control_high_water(&config, &fixture)?;

    // When
    let restarted = Runtime::start_with_clock(config.clone(), support::clock()).await?;
    let second_id = restarted.handle().status().await?.endpoint_id();
    restarted.shutdown().await?;

    // Then
    let after = persisted_control_high_water(&config, &fixture)?;
    assert_eq!(second_id, first_id);
    assert_eq!(after, before);
    assert!(before.iter().all(|state| state.manifest_generation > 0));
    assert!(before.iter().all(|state| state.address_sequence > 0));
    assert!(before.iter().all(|state| state.relay_sequence > 0));
    let before_evidence = before
        .iter()
        .map(PersistedControlHighWater::evidence)
        .collect::<Vec<_>>();
    let after_evidence = after
        .iter()
        .map(PersistedControlHighWater::evidence)
        .collect::<Vec<_>>();
    emit(&serde_json::json!({
        "scenario": "persistent-identity-restart",
        "endpoint_ids": {
            "before": first_id.to_public_key()?.to_string(),
            "after": second_id.to_public_key()?.to_string()
        },
        "control_high_water_before": before_evidence,
        "control_high_water_after": after_evidence
    }));
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct PersistedControlHighWater {
    space_id: String,
    manifest_generation: u64,
    manifest_hash: String,
    address_sequence: u64,
    address_hash: String,
    relay_sequence: u64,
    relay_hash: String,
}

impl PersistedControlHighWater {
    fn evidence(&self) -> serde_json::Value {
        serde_json::json!({
            "space_id": self.space_id,
            "manifest_generation": self.manifest_generation,
            "manifest_hash": self.manifest_hash,
            "address_sequence": self.address_sequence,
            "address_hash": self.address_hash,
            "relay_sequence": self.relay_sequence,
            "relay_hash": self.relay_hash
        })
    }
}

fn persisted_control_high_water(
    config: &StoreConfig,
    fixture: &support::ControlFixture,
) -> Result<Vec<PersistedControlHighWater>, Box<dyn std::error::Error + Send + Sync>> {
    let repository = Repository::open(config)?;
    fixture
        .shared_spaces
        .iter()
        .map(|space_id| {
            let chain = repository
                .load_space_chain(*space_id)?
                .ok_or("persisted manifest high-water missing")?;
            let address = repository
                .address_record(*space_id, fixture.owner_secret.public().into())?
                .ok_or("persisted address high-water missing")?;
            let relay = repository
                .relay_advertisement(*space_id, fixture.owner_secret.public().into())?
                .ok_or("persisted relay high-water missing")?;
            Ok(PersistedControlHighWater {
                space_id: format!("{space_id:?}"),
                manifest_generation: chain.latest_generation(),
                manifest_hash: hex(&chain.latest_hash()),
                address_sequence: address.sequence(),
                address_hash: hex(&address.record_hash()),
                relay_sequence: relay.sequence(),
                relay_hash: hex(&relay.advertisement_hash()),
            })
        })
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut output, byte| {
        let _result = write!(output, "{byte:02x}");
        output
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signer_claim_mismatches_preserve_address_and_relay_high_water() -> TestResult {
    // Given
    let fixture = support::control_fixture().await?;
    let config = fixture.candidate_config();
    let mut repository = Repository::open(&config)?;
    let space_id = fixture.shared_spaces[0];
    let endpoint_c = SecretKey::from_bytes(&[0x6c; 32]);
    crate::control_sync_e2e_artifacts::persist_relay(
        &mut repository,
        fixture
            .owner_advertisements
            .first()
            .ok_or("owner relay fixture missing")?,
        1_700_000_000_000,
    )?;
    let address_before = repository
        .address_record(space_id, fixture.owner_secret.public().into())?
        .ok_or("address high-water missing")?
        .sequence();
    let relay_before = repository
        .relay_advertisement(space_id, fixture.owner_secret.public().into())?
        .ok_or("relay high-water missing")?
        .sequence();
    let revision_before = repository.revision()?;

    // When
    let address = SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, endpoint_c.public().into()),
        AddressRecordValidity::new(99, 1_700_000_000_000, 1_700_000_600_000)?,
        AddressEndpointDataV1::new(Vec::new())?,
    )
    .sign(&fixture.candidate_secret);
    let relay = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(space_id, fixture.candidate_secret.public().into()),
        "https://mismatch.example.invalid".parse()?,
        PrivateRelayAdvertisementValidity::new(99, 1_700_000_000_000, 1_700_000_600_000)?,
    )?
    .sign(&fixture.owner_secret);

    // Then
    assert!(address.is_err());
    assert!(relay.is_err());
    let repository = Repository::open(&config)?;
    assert_eq!(repository.revision()?, revision_before);
    assert_eq!(
        repository
            .address_record(space_id, fixture.owner_secret.public().into())?
            .ok_or("address high-water missing after rejection")?
            .sequence(),
        address_before
    );
    assert_eq!(
        repository
            .relay_advertisement(space_id, fixture.owner_secret.public().into())?
            .ok_or("relay high-water missing after rejection")?
            .sequence(),
        relay_before
    );
    emit(&serde_json::json!({
        "scenario": "signer-claim-mismatch",
        "endpoint_ids": {
            "address_signer_b": fixture.candidate_secret.public().to_string(),
            "address_claimed_c": endpoint_c.public().to_string(),
            "relay_signer_a": fixture.owner_secret.public().to_string(),
            "relay_claimed_b": fixture.candidate_secret.public().to_string()
        },
        "accepted": false,
        "address_high_water": address_before,
        "relay_high_water": relay_before,
        "revision": revision_before
    }));
    Ok(())
}
