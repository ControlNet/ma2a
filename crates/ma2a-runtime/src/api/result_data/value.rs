//! JSON encoders for the typed local API result payloads.

use serde_json::{Value, json};

use super::{
    CapabilityFlags, EchoReplyView, HandshakeView, PrivateRelayView, PublicRelayView,
    RuntimeStatusView, UiOpenView,
};
use crate::api::{codec_fields::encode_hex, snapshot::UiAuthView};

pub(crate) fn handshake_value(value: &HandshakeView) -> Value {
    json!({
        "runtime_version": value.runtime_version,
        "endpoint_id": encode_hex(value.endpoint_id.as_bytes()),
        "revision": value.revision,
        "initialized": value.initialized,
        "password_set": value.password_set,
        "capabilities": capability_value(value.capabilities),
    })
}

pub(crate) fn status_value(value: RuntimeStatusView) -> Value {
    json!({"revision": value.revision, "initialized": value.initialized, "shutting_down": value.shutting_down})
}

pub(crate) fn private_relay_value(value: &PrivateRelayView) -> Value {
    json!({"configured": value.configured, "mode": value.mode, "host": value.host, "port": value.port, "online": value.online})
}

pub(crate) fn public_relay_value(value: &PublicRelayView) -> Value {
    json!({"configured": value.configured, "url": value.url, "online": value.online})
}

pub(crate) fn ui_open_value(value: &UiOpenView) -> Value {
    json!({"url": value.url})
}

pub(crate) fn echo_reply_value(value: &EchoReplyView) -> Value {
    json!({"target_endpoint_id": encode_hex(value.target_endpoint_id.as_bytes()), "payload": value.payload, "duration_ms": value.duration_ms})
}

pub(crate) fn ui_auth_result_value(value: &UiAuthView) -> Value {
    crate::api::snapshot::ui_auth_value(value)
}

fn capability_value(value: CapabilityFlags) -> Value {
    json!({"spaces": value.management.spaces, "control_sync": value.management.control_sync, "private_relay": value.relays.private_relay, "public_relay": value.relays.public_relay, "echo": value.interaction.echo, "events": value.interaction.events})
}
