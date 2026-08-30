use std::collections::BTreeSet;

use ma2a_core::EndpointId;

const MAX_PEERS_PER_SPACE: usize = 4;

/// Selects a deduplicated rotating window of at most four peers.
pub fn select_peer_window(
    local_endpoint_id: EndpointId,
    peers: &[EndpointId],
    rotation: usize,
) -> Vec<EndpointId> {
    let peers = peers
        .iter()
        .copied()
        .filter(|peer| *peer != local_endpoint_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if peers.is_empty() {
        return Vec::new();
    }
    let start = rotation % peers.len();
    (0..peers.len().min(MAX_PEERS_PER_SPACE))
        .filter_map(|offset| peers.get((start + offset) % peers.len()).copied())
        .collect()
}
