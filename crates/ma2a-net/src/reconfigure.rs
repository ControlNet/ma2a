use std::{collections::BTreeSet, error::Error, fmt};

use iroh::Endpoint;
use iroh_base::RelayUrl;

use crate::LocalIrohRelayMap;

/// Result of one Iroh-supported relay-map mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayReconfigureOutcome {
    changed: bool,
    endpoint_id: ma2a_core::EndpointId,
}

impl RelayReconfigureOutcome {
    /// Returns whether the supplied URL set materially changed.
    pub const fn changed(self) -> bool {
        self.changed
    }
    /// Returns the unchanged Endpoint identity observed after mutation.
    pub const fn endpoint_id(self) -> ma2a_core::EndpointId {
        self.endpoint_id
    }
}

/// Relay mutation failed because the live Endpoint closed or changed identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RelayReconfigureError;

impl fmt::Display for RelayReconfigureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Iroh relay reconfiguration failed")
    }
}

impl Error for RelayReconfigureError {}

pub(crate) async fn apply_relay_map(
    endpoint: &Endpoint,
    configured: &mut BTreeSet<RelayUrl>,
    relay_map: &LocalIrohRelayMap,
) -> Result<RelayReconfigureOutcome, RelayReconfigureError> {
    if endpoint.is_closed() {
        return Err(RelayReconfigureError);
    }
    let identity = endpoint.id();
    let desired = relay_map.relay_urls().cloned().collect::<BTreeSet<_>>();
    if desired == *configured {
        return Ok(RelayReconfigureOutcome {
            changed: false,
            endpoint_id: identity.into(),
        });
    }
    let configurations = relay_map.relay_map();
    for relay_url in desired.difference(configured) {
        let configuration = configurations.get(relay_url).ok_or(RelayReconfigureError)?;
        endpoint
            .insert_relay(relay_url.clone(), configuration)
            .await;
    }
    for relay_url in configured.difference(&desired) {
        endpoint.remove_relay(relay_url).await;
    }
    if endpoint.is_closed() || endpoint.id() != identity {
        return Err(RelayReconfigureError);
    }
    *configured = desired;
    Ok(RelayReconfigureOutcome {
        changed: true,
        endpoint_id: identity.into(),
    })
}
