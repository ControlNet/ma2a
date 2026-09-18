use serde_json::{Value, json};

use super::{
    ConnectionObservationView, ConnectionView, ControlRoundView, EndpointView,
    PrivateRelayCandidateView, PublicRelayFallbackView, RuntimeSnapshot,
    SnapshotSpaceView, SpaceMemberView, SpaceView, UiAuthView,
};
use crate::api::codec_fields::encode_hex;

impl RuntimeSnapshot {
    pub(crate) fn to_value(&self) -> Value {
        json!({
            "revision": self.revision,
            "endpoint": endpoint_value(&self.endpoint),
            "spaces": self.spaces.iter().map(snapshot_space_value).collect::<Vec<_>>(),
            "control_sync": {"peer_endpoint_ids": self.control_sync.peers.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>()},
            "connections": self.connections.iter().map(connection_value).collect::<Vec<_>>(),
            "private_relay_candidates": self.private_relay_candidates.iter().map(private_relay_value).collect::<Vec<_>>(),
            "public_relay_fallbacks": self.public_relay_fallbacks.iter().map(public_relay_value).collect::<Vec<_>>(),
            "control_rounds": self.control_rounds.iter().map(control_round_value).collect::<Vec<_>>(),
            "observed_relay_state": {"private_relay_online": self.observed_relay_state.private_relay_online, "public_relay_online": self.observed_relay_state.public_relay_online},
            "reachability": {"state": self.reachability.state, "direct": self.reachability.direct, "relayed": self.reachability.relayed},
            "recent_echo_summary": {"successes": self.recent_echo_summary.successes, "failures": self.recent_echo_summary.failures},
            "ui_auth": ui_auth_value(&self.ui_auth),
        })
    }
}

fn connection_value(connection: &ConnectionView) -> Value {
    json!({
        "endpoint_id": encode_hex(connection.endpoint_id.as_bytes()),
        "state": connection.state,
        "path": connection.path,
        "rtt_ms": connection.rtt_ms,
        "observations": connection.observations.iter().map(observation_value).collect::<Vec<_>>(),
    })
}

fn observation_value(observation: &ConnectionObservationView) -> Value {
    json!({
        "observed_at_ms": observation.observed_at_ms,
        "path": observation.path,
        "rtt_ms": observation.rtt_ms,
        "error_class": observation.error_class,
    })
}

fn member_value(member: &SpaceMemberView) -> Value {
    json!({
        "endpoint_id": encode_hex(member.endpoint_id.as_bytes()),
        "label": member.label,
        "echo": member.echo,
        "relay_provider": member.relay_provider,
    })
}

fn snapshot_space_value(space: &SnapshotSpaceView) -> Value {
    json!({
        "space_id": encode_hex(space.id.as_bytes()),
        "name": space.name,
        "member_count": space.member_count,
        "generation": space.generation,
        "chain_hash": encode_hex(&space.chain_hash),
        "members": space.members.iter().map(member_value).collect::<Vec<_>>(),
        "revoked_count": space.revoked_count,
    })
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

fn private_relay_value(candidate: &PrivateRelayCandidateView) -> Value {
    json!({
        "provider_endpoint_id": encode_hex(candidate.provider_endpoint_id.as_bytes()),
        "relay_url": candidate.relay_url,
        "covered_space_ids": candidate.covered_space_ids.iter().map(|id| encode_hex(id.as_bytes())).collect::<Vec<_>>(),
        "home_compatible": candidate.home_compatible,
    })
}

fn public_relay_value(fallback: &PublicRelayFallbackView) -> Value {
    json!({
        "relay_url": fallback.relay_url,
        "enabled": fallback.enabled,
        "observed_connected": fallback.observed_connected,
    })
}

fn control_round_value(round: &ControlRoundView) -> Value {
    json!({"at_ms": round.at_ms, "peer_count": round.peer_count, "outcome": round.outcome})
}
