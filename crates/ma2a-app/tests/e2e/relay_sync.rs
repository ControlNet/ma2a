use std::time::Duration;

use iroh::{Endpoint, RelayMode, SecretKey, endpoint::presets};
use ma2a_core::SignedSpaceAddressRecordV1;
use ma2a_net::{AddressPublishRequest, AddressPublisher};
use ma2a_runtime::Runtime;
use ma2a_store::Repository;

use super::harness::{NOW_MS, TestResult, emit, wait_for_home_status};
use crate::control_sync_e2e_support as support;

struct AddressPublications<'a> {
    config: &'a ma2a_store::StoreConfig,
    spaces: &'a [ma2a_core::SpaceId; 2],
    secret: &'a SecretKey,
}

struct SynchronizedRecords<'a> {
    config: &'a ma2a_store::StoreConfig,
    spaces: &'a [ma2a_core::SpaceId; 2],
    endpoint_id: ma2a_core::EndpointId,
    first_sequences: &'a [u64],
    second_sequences: &'a [u64],
    expected_sequences: &'a [u64],
    expected_addresses: &'a std::collections::BTreeSet<iroh_base::TransportAddr>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scenario_f_observed_home_change_propagates_through_control_sync() -> TestResult {
    // Given
    let fixture = support::control_fixture().await?;
    let owner_config = fixture.owner_config();
    let candidate_config = fixture.candidate_config();
    let bind_port = Repository::open(&owner_config)?
        .endpoint_bind_port()?
        .ok_or("owner bind port missing")?;
    let (first_map, first_url, first_relay) = iroh::test_utils::run_relay_server().await?;
    let (second_map, second_url, second_relay) = iroh::test_utils::run_relay_server().await?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(fixture.owner_secret.clone())
        .bind_addr((std::net::Ipv4Addr::UNSPECIFIED, bind_port))?
        .relay_mode(RelayMode::Custom(first_map))
        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    assert!(wait_for_home_status(&endpoint, &first_url).await?);
    let first_addr = endpoint.addr();
    let publications = AddressPublications {
        config: &owner_config,
        spaces: &fixture.shared_spaces,
        secret: &fixture.owner_secret,
    };
    let first_sequences = publications.publish(&first_addr)?;
    let second_config = second_map
        .get(&second_url)
        .ok_or("second relay configuration missing")?;

    // When
    endpoint
        .insert_relay(second_url.clone(), second_config)
        .await;
    endpoint.remove_relay(&first_url).await;
    assert!(wait_for_home_status(&endpoint, &second_url).await?);
    let second_addr = endpoint.addr();
    let second_sequences = publications.publish(&second_addr)?;
    endpoint.close().await;
    drop(endpoint);
    let candidate = Runtime::start_with_clock(candidate_config.clone(), support::clock()).await?;
    assert!(
        candidate
            .connections()
            .observations(fixture.owner_secret.public().into())
            .iter()
            .all(|observation| observation.last_success_at_ms().is_none())
    );
    let owner = Runtime::start_with_clock(owner_config.clone(), support::clock()).await?;
    let owner_status = owner.handle().status().await?;
    let owner_sequences = address_sequences(
        &owner_config,
        &fixture.shared_spaces,
        fixture.owner_secret.public().into(),
    )?;
    let owner_addresses = owner_status.endpoint_addr().addrs;
    let sync_revision =
        tokio::time::timeout(Duration::from_secs(15), candidate.handle().sync_control()).await??;
    let observations = candidate
        .connections()
        .observations(fixture.owner_secret.public().into());

    // Then
    assert_ne!(first_addr.addrs, second_addr.addrs);
    assert_eq!(first_sequences.len(), fixture.shared_spaces.len());
    assert_eq!(second_sequences.len(), fixture.shared_spaces.len());
    let received = SynchronizedRecords {
        config: &candidate_config,
        spaces: &fixture.shared_spaces,
        endpoint_id: fixture.owner_secret.public().into(),
        first_sequences: &first_sequences,
        second_sequences: &second_sequences,
        expected_sequences: &owner_sequences,
        expected_addresses: &owner_addresses,
    }
    .assert_exact()?;
    emit(&serde_json::json!({
        "scenario": "F",
        "endpoint_ids": {
            "source": fixture.owner_secret.public().to_string(),
            "receiver": fixture.candidate_secret.public().to_string()
        },
        "record_sequences_before": first_sequences,
        "record_sequences_after": second_sequences,
        "record_sequences": second_sequences,
        "supplied_relay_candidates": [second_url.to_string()],
        "iroh_observed_effective_home": second_url.to_string(),
        "iroh_observed_path": second_url.to_string(),
        "reachability_state": "ControlSyncPropagatedAfterHomeChange",
        "owner_runtime_high_water": owner_sequences,
        "owner_runtime_endpoint_id": owner_status.endpoint_id().to_public_key()?.to_string(),
        "sync_revision": sync_revision,
        "connection_observations": observations.len(),
        "durable_received_high_water": received,
        "observed_home_before": first_url.to_string(),
        "observed_home_after": second_url.to_string(),
        "control_sync_propagated": true
    }));
    candidate.shutdown().await?;
    owner.shutdown().await?;
    drop(first_relay);
    drop(second_relay);
    Ok(())
}

fn address_sequences(
    config: &ma2a_store::StoreConfig,
    spaces: &[ma2a_core::SpaceId; 2],
    endpoint_id: ma2a_core::EndpointId,
) -> Result<Vec<u64>, Box<dyn std::error::Error + Send + Sync>> {
    let repository = Repository::open(config)?;
    spaces
        .iter()
        .map(|space_id| {
            repository
                .address_record(*space_id, endpoint_id)?
                .map(|record| record.sequence())
                .ok_or_else(|| "owner runtime address missing".into())
        })
        .collect()
}

impl AddressPublications<'_> {
    fn publish(
        &self,
        endpoint_addr: &iroh::EndpointAddr,
    ) -> Result<Vec<u64>, Box<dyn std::error::Error + Send + Sync>> {
        let mut repository = Repository::open(self.config)?;
        let mut sequences = Vec::new();
        for space_id in self.spaces {
            let chain = repository
                .load_space_chain(*space_id)?
                .ok_or("owner chain missing")?;
            let authorization = ma2a_core::SpaceAuthorizationView::from_chain(&chain);
            let published = AddressPublisher::new(self.secret.clone(), endpoint_addr.clone())?
                .publish(
                    &mut repository,
                    AddressPublishRequest::new(&authorization, NOW_MS),
                )?
                .ok_or("changed address publication missing")?;
            sequences.push(published.record().record().sequence());
        }
        Ok(sequences)
    }
}

impl SynchronizedRecords<'_> {
    fn assert_exact(&self) -> Result<Vec<u64>, Box<dyn std::error::Error + Send + Sync>> {
        let repository = Repository::open(self.config)?;
        let mut received = Vec::new();
        for (((space_id, first_sequence), second_sequence), expected_sequence) in self
            .spaces
            .iter()
            .zip(self.first_sequences)
            .zip(self.second_sequences)
            .zip(self.expected_sequences)
        {
            assert_eq!(*second_sequence, first_sequence + 1);
            assert_eq!(*expected_sequence, second_sequence + 1);
            let record = repository
                .address_record(*space_id, self.endpoint_id)?
                .ok_or("synchronized changed address missing")?;
            let signed = SignedSpaceAddressRecordV1::parse_canonical_bytes(record.signed_record())?;
            let synchronized_addresses = signed
                .record()
                .endpoint_data()
                .addresses()
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            assert_eq!(record.sequence(), *expected_sequence);
            assert_eq!(&synchronized_addresses, self.expected_addresses);
            received.push(record.sequence());
        }
        Ok(received)
    }
}
