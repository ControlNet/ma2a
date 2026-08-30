use iroh::{Endpoint, EndpointAddr, Watcher as _};
use iroh_base::RelayUrl;
use ma2a_core::AddressEndpointDataV1;
use tokio::sync::mpsc;

use crate::address_observation::AddressObservation;

/// One Iroh-reported home relay connection state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrohHomeRelayObservation {
    relay_url: RelayUrl,
    connected: bool,
}

impl IrohHomeRelayObservation {
    /// Creates one observed home-relay connection state.
    pub const fn new(relay_url: RelayUrl, connected: bool) -> Self {
        Self {
            relay_url,
            connected,
        }
    }

    /// Returns the home relay URL selected by Iroh.
    pub const fn relay_url(&self) -> &RelayUrl {
        &self.relay_url
    }

    /// Returns whether Iroh completed the relay connection handshake.
    pub const fn is_connected(&self) -> bool {
        self.connected
    }
}

/// Current effective Iroh address and home-relay observations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrohRelayObservation {
    endpoint_addr: EndpointAddr,
    endpoint_data: AddressEndpointDataV1,
    home_relays: Vec<IrohHomeRelayObservation>,
}

impl IrohRelayObservation {
    /// Creates one complete effective endpoint and home-relay observation.
    ///
    /// # Errors
    /// Returns [`ma2a_core::ProtocolError::INVALID_INPUT`] for an invalid endpoint identity.
    pub fn new(
        endpoint_id: ma2a_core::EndpointId,
        endpoint_data: AddressEndpointDataV1,
        home_relays: Vec<IrohHomeRelayObservation>,
    ) -> Result<Self, ma2a_core::ProtocolError> {
        Ok(Self {
            endpoint_addr: EndpointAddr::from_parts(
                endpoint_id.to_public_key()?,
                endpoint_data.addresses().iter().cloned(),
            ),
            endpoint_data,
            home_relays,
        })
    }

    /// Returns Iroh's actual current direct and relay addressing.
    pub const fn endpoint_addr(&self) -> &EndpointAddr {
        &self.endpoint_addr
    }

    /// Returns the complete bounded data used for signed address publication.
    pub const fn endpoint_data(&self) -> &AddressEndpointDataV1 {
        &self.endpoint_data
    }

    /// Returns every home relay currently reported by Iroh.
    pub fn home_relays(&self) -> &[IrohHomeRelayObservation] {
        &self.home_relays
    }
}

#[derive(Clone, Debug)]
pub(crate) struct IrohRelayObserver {
    endpoint: Endpoint,
    address_observation: AddressObservation,
}

impl IrohRelayObserver {
    pub(crate) const fn new(endpoint: Endpoint, address_observation: AddressObservation) -> Self {
        Self {
            endpoint,
            address_observation,
        }
    }

    pub(crate) async fn run(self, sender: mpsc::Sender<IrohRelayObservation>) {
        let mut address_generation = self.address_observation.subscribe();
        let mut home_watcher = self.endpoint.home_relay_status();
        let Ok((_, endpoint_data)) = self.address_observation.current() else {
            return;
        };
        let mut last = observation(&self.endpoint, endpoint_data, home_watcher.get());
        if sender.send(last.clone()).await.is_err() {
            return;
        }
        loop {
            let next = tokio::select! {
                () = self.endpoint.closed() => break,
                changed = address_generation.changed() => match changed {
                    Ok(()) => match self.address_observation.current() {
                        Ok((_, endpoint_data)) => observation(
                            &self.endpoint,
                            endpoint_data,
                            home_watcher.get(),
                        ),
                        Err(_) => break,
                    },
                    Err(_) => break,
                },
                homes = home_watcher.updated() => match homes {
                    Ok(homes) => match self.address_observation.current() {
                        Ok((_, endpoint_data)) => observation(&self.endpoint, endpoint_data, homes),
                        Err(_) => break,
                    },
                    Err(_) => break,
                },
            };
            if next == last {
                continue;
            }
            last = next.clone();
            if sender.send(next).await.is_err() {
                break;
            }
        }
    }
}

fn observation(
    endpoint: &Endpoint,
    endpoint_data: AddressEndpointDataV1,
    statuses: Vec<iroh::endpoint::RelayStatus>,
) -> IrohRelayObservation {
    let mut home_relays = statuses
        .into_iter()
        .map(|status| IrohHomeRelayObservation {
            relay_url: status.url().clone(),
            connected: status.is_connected(),
        })
        .collect::<Vec<_>>();
    home_relays
        .sort_unstable_by(|left, right| left.relay_url.as_str().cmp(right.relay_url.as_str()));
    IrohRelayObservation {
        endpoint_addr: EndpointAddr::from_parts(
            endpoint.id(),
            endpoint_data.addresses().iter().cloned(),
        ),
        endpoint_data,
        home_relays,
    }
}
