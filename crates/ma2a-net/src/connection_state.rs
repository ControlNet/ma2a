use iroh::{Endpoint, EndpointAddr, Watcher as _};
use iroh_base::RelayUrl;
use tokio::sync::mpsc;

/// One Iroh-reported home relay connection state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrohHomeRelayObservation {
    relay_url: RelayUrl,
    connected: bool,
}

impl IrohHomeRelayObservation {
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
    home_relays: Vec<IrohHomeRelayObservation>,
}

impl IrohRelayObservation {
    /// Returns Iroh's actual current direct and relay addressing.
    pub const fn endpoint_addr(&self) -> &EndpointAddr {
        &self.endpoint_addr
    }

    /// Returns every home relay currently reported by Iroh.
    pub fn home_relays(&self) -> &[IrohHomeRelayObservation] {
        &self.home_relays
    }
}

#[derive(Clone, Debug)]
pub(crate) struct IrohRelayObserver {
    endpoint: Endpoint,
}

impl IrohRelayObserver {
    pub(crate) const fn new(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }

    pub(crate) async fn run(self, sender: mpsc::Sender<IrohRelayObservation>) {
        let mut address_watcher = self.endpoint.watch_addr();
        let mut home_watcher = self.endpoint.home_relay_status();
        if sender
            .send(observation(address_watcher.get(), home_watcher.get()))
            .await
            .is_err()
        {
            return;
        }
        loop {
            let next = tokio::select! {
                () = self.endpoint.closed() => break,
                address = address_watcher.updated() => match address {
                    Ok(address) => observation(address, home_watcher.get()),
                    Err(_) => break,
                },
                homes = home_watcher.updated() => match homes {
                    Ok(homes) => observation(address_watcher.get(), homes),
                    Err(_) => break,
                },
            };
            if sender.send(next).await.is_err() {
                break;
            }
        }
    }
}

fn observation(
    endpoint_addr: EndpointAddr,
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
        endpoint_addr,
        home_relays,
    }
}
