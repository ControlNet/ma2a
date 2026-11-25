use std::{io, net::SocketAddr, time::Duration};

use iroh::{
    Endpoint, RelayMode,
    address_lookup::{EndpointData, memory::MemoryLookup},
    endpoint::presets,
};
use iroh_base::TransportAddr;
use tokio::sync::mpsc;

use super::IrohRelayObserver;
use crate::address_observation::AddressObservation;

type TestResult = Result<(), io::Error>;

/// `count` loopback addresses on consecutive ports from `first_port`.
fn addresses(first_port: u16, count: u16) -> EndpointData {
    EndpointData::new(
        (0..count)
            .map(|offset| {
                TransportAddr::Ip(SocketAddr::from(([127, 0, 0, 1], first_port + offset)))
            })
            .collect(),
    )
}

fn ports(addresses: &[TransportAddr]) -> Vec<u16> {
    addresses
        .iter()
        .filter_map(|address| match address {
            TransportAddr::Ip(address) => Some(address.port()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn too_many_local_addresses_for_a_while_do_not_end_observation() -> TestResult {
    // Given: an observer that reported one valid address set.
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .address_lookup(MemoryLookup::with_provenance("ma2a_relay_observer"))
        .bind()
        .await
        .map_err(io::Error::other)?;
    let observation = AddressObservation::default();
    observation.observe(&addresses(4_900, 1));
    let (sender, mut observations) = mpsc::channel(4);
    let observer =
        tokio::spawn(IrohRelayObserver::new(endpoint.clone(), observation.clone()).run(sender));
    observations
        .recv()
        .await
        .ok_or_else(|| io::Error::other("first observation missing"))?;

    // When: the host briefly has more addresses than a record may carry.
    observation.observe(&addresses(4_910, 17));
    let during = tokio::time::timeout(Duration::from_millis(200), observations.recv()).await;
    observation.observe(&addresses(4_930, 1));

    // Then: nothing is reported for the invalid set, and the next valid one still is.
    assert!(during.is_err(), "observer reported or stopped: {during:?}");
    let next = tokio::time::timeout(Duration::from_secs(5), observations.recv())
        .await
        .map_err(io::Error::other)?
        .ok_or_else(|| io::Error::other("observer stopped"))?;
    assert_eq!(ports(next.endpoint_data().addresses()), vec![4_930]);
    observer.abort();
    endpoint.close().await;
    Ok(())
}
