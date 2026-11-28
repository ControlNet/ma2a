use ma2a_net::RuntimeEndpoint;

use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    state::{Connectivity, RuntimeStatus},
};

pub(super) fn initial_status(
    (endpoint_id, memberships): (
        ma2a_core::EndpointId,
        std::collections::BTreeSet<ma2a_core::SpaceId>,
    ),
    (endpoint, endpoint_data): (&RuntimeEndpoint, ma2a_core::AddressEndpointDataV1),
    (boot_id, revision, relay_map): ([u8; 16], u64, ma2a_net::LocalIrohRelayMap),
) -> RuntimeStatus {
    let endpoint_addr = endpoint.endpoint_addr();
    let direct_reachable = endpoint_addr.ip_addrs().next().is_some();
    RuntimeStatus {
        endpoint_id,
        endpoint_addr,
        endpoint_data,
        boot_id,
        revision,
        memberships,
        ready: true,
        connectivity: Connectivity::DIRECT_ONLY,
        direct_reachable,
        relay: crate::reachability::RelayReachabilityState::new(relay_map),
    }
}

pub(super) fn boot_id() -> Result<[u8; 16], RuntimeError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RuntimeError::new(RuntimeErrorKind::Random))?;
    Ok(bytes)
}
