use serde_json::{Value, json};

use super::{
    ConnectionView, EndpointView, RelayCandidateView, RuntimeSnapshot, SpaceView, UiAuthView,
};
use crate::api::codec_fields::encode_hex;

impl RuntimeSnapshot {
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "revision": self.revision,
            "endpoint": endpoint_value(&self.endpoint),
            "spaces": self.spaces.iter().map(space_value).collect::<Vec<_>>(),
            "control_sync": {"peer_endpoint_ids": self.control_sync.peers.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>()},
            "connections": self.connections.iter().map(connection_value).collect::<Vec<_>>(),
            "relay_candidates": self.relay_candidates.iter().map(relay_candidate_value).collect::<Vec<_>>(),
            "observed_relay_state": {"private_relay_online": self.observed_relay_state.private_relay_online, "public_relay_online": self.observed_relay_state.public_relay_online},
            "reachability": {"direct": self.reachability.direct, "relayed": self.reachability.relayed},
            "recent_echo_summary": {"successes": self.recent_echo_summary.successes, "failures": self.recent_echo_summary.failures},
            "ui_auth": ui_auth_value(&self.ui_auth),
        })
    }
}

fn connection_value(connection: &ConnectionView) -> Value {
    json!({"endpoint_id": encode_hex(connection.endpoint_id.as_bytes()), "state": connection.state, "path": connection.path, "rtt_ms": connection.rtt_ms})
}

pub(crate) fn endpoint_value(endpoint: &EndpointView) -> Value {
    json!({"endpoint_id": encode_hex(endpoint.id.as_bytes()), "runtime_version": endpoint.runtime_version, "online": endpoint.online})
}

pub(crate) fn space_value(space: &SpaceView) -> Value {
    json!({"space_id": encode_hex(space.id.as_bytes()), "name": space.name, "member_count": space.member_count})
}

pub(crate) fn ui_auth_value(auth: &UiAuthView) -> Value {
    json!({"initialized": auth.initialized, "password_set": auth.password_set, "active_sessions": auth.active_sessions})
}

fn relay_candidate_value(candidate: &RelayCandidateView) -> Value {
    json!({"endpoint_id": encode_hex(candidate.endpoint_id.as_bytes()), "relay_kind": candidate.relay_kind, "eligible": candidate.eligible})
}
