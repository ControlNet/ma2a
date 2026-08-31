//! Typed local API result payloads.

use ma2a_core::EndpointId;
use serde_json::{Value, json};

use super::{codec_fields::encode_hex, snapshot::UiAuthView};

/// Phase 1 local API capabilities advertised without Space details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityFlags {
    management: ManagementCapabilities,
    relays: RelayCapabilities,
    interaction: InteractionCapabilities,
}

impl CapabilityFlags {
    /// Creates an explicit capability set.
    pub const fn new(
        management: ManagementCapabilities,
        relays: RelayCapabilities,
        interaction: InteractionCapabilities,
    ) -> Self {
        Self {
            management,
            relays,
            interaction,
        }
    }
}

/// Space and control-sync capability group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagementCapabilities {
    spaces: bool,
    control_sync: bool,
}

impl ManagementCapabilities {
    /// Creates explicit management capabilities.
    pub const fn new(spaces: bool, control_sync: bool) -> Self {
        Self {
            spaces,
            control_sync,
        }
    }
}

/// Relay capabilities grouped for capability construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayCapabilities {
    private_relay: bool,
    public_relay: bool,
}

impl RelayCapabilities {
    /// Creates explicit Private and Public Relay capabilities.
    pub const fn new(private_relay: bool, public_relay: bool) -> Self {
        Self {
            private_relay,
            public_relay,
        }
    }
}

/// Echo and event-stream capability group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionCapabilities {
    echo: bool,
    events: bool,
}

impl InteractionCapabilities {
    /// Creates explicit interaction capabilities.
    pub const fn new(echo: bool, events: bool) -> Self {
        Self { echo, events }
    }
}

/// Pre-authorization handshake data with no Space details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeView {
    pub(crate) runtime_version: String,
    pub(crate) endpoint_id: EndpointId,
    pub(crate) revision: u64,
    pub(crate) initialized: bool,
    pub(crate) password_set: bool,
    pub(crate) capabilities: CapabilityFlags,
}

impl HandshakeView {
    /// Creates a bounded pre-authorization handshake result.
    ///
    /// # Errors
    /// Returns invalid input when the Runtime version is empty or exceeds 128 bytes.
    pub fn new(
        runtime_version: &str,
        endpoint_id: EndpointId,
        state: HandshakeState,
    ) -> Result<Self, super::ApiError> {
        if runtime_version.is_empty() || runtime_version.len() > 128 {
            Err(super::ApiError::invalid_input())
        } else {
            Ok(Self {
                runtime_version: runtime_version.to_owned(),
                endpoint_id,
                revision: state.revision,
                initialized: state.auth.initialized,
                password_set: state.auth.password_set,
                capabilities: state.capabilities,
            })
        }
    }
}

/// Handshake initialization and password status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeAuth {
    initialized: bool,
    password_set: bool,
}

impl HandshakeAuth {
    /// Creates public handshake authentication status.
    pub const fn new(initialized: bool, password_set: bool) -> Self {
        Self {
            initialized,
            password_set,
        }
    }
}

/// Revision, authentication status, and capabilities grouped for handshake construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeState {
    revision: u64,
    auth: HandshakeAuth,
    capabilities: CapabilityFlags,
}

impl HandshakeState {
    /// Creates grouped handshake state.
    pub const fn new(revision: u64, auth: HandshakeAuth, capabilities: CapabilityFlags) -> Self {
        Self {
            revision,
            auth,
            capabilities,
        }
    }
}

/// Current Runtime lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStatusView {
    pub(crate) revision: u64,
    pub(crate) initialized: bool,
    pub(crate) shutting_down: bool,
}

impl RuntimeStatusView {
    /// Creates current Runtime lifecycle status.
    pub const fn new(revision: u64, initialized: bool, shutting_down: bool) -> Self {
        Self {
            revision,
            initialized,
            shutting_down,
        }
    }
}

/// Private Relay configuration and observed status without key material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateRelayView {
    pub(crate) configured: bool,
    pub(crate) mode: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) online: bool,
}

impl PrivateRelayView {
    /// Creates a Private Relay status result.
    pub fn new(configured: bool, address: RelayAddress, online: bool) -> Self {
        Self {
            configured,
            mode: address.mode,
            host: address.host,
            port: address.port,
            online,
        }
    }
}

/// Bounded Private Relay mode and listen address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayAddress {
    mode: String,
    host: String,
    port: u16,
}

impl RelayAddress {
    /// Creates a bounded relay address.
    ///
    /// # Errors
    /// Returns invalid input for an unknown mode or invalid host length.
    pub fn new(mode: &str, host: &str, port: u16) -> Result<Self, super::ApiError> {
        if !matches!(mode, "native_tls" | "external_termination")
            || host.is_empty()
            || host.len() > 253
        {
            Err(super::ApiError::invalid_input())
        } else {
            Ok(Self {
                mode: mode.to_owned(),
                host: host.to_owned(),
                port,
            })
        }
    }
}

/// Public Relay configuration and observed status without URL credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRelayView {
    pub(crate) configured: bool,
    pub(crate) url: Option<String>,
    pub(crate) online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Credential-free loopback Web endpoint owned by the daemon.
pub struct UiOpenView {
    pub(crate) url: String,
}

impl UiOpenView {
    /// Creates a bounded IPv4 loopback URL.
    ///
    /// # Errors
    /// Returns invalid input for non-loopback or oversized URLs.
    pub fn new(url: &str) -> Result<Self, super::ApiError> {
        if !url.starts_with("http://127.0.0.1:") || url.len() > 64 {
            Err(super::ApiError::invalid_input())
        } else {
            Ok(Self {
                url: url.to_owned(),
            })
        }
    }
}

impl PublicRelayView {
    /// Creates Public Relay configuration and observed status.
    ///
    /// # Errors
    /// Returns invalid input when the URL is oversized or contains credentials.
    pub fn new(
        configured: bool,
        url: Option<String>,
        online: bool,
    ) -> Result<Self, super::ApiError> {
        if url
            .as_ref()
            .is_some_and(|value| value.len() > 2_048 || value.contains('@'))
        {
            Err(super::ApiError::invalid_input())
        } else {
            Ok(Self {
                configured,
                url,
                online,
            })
        }
    }
}

/// Bounded Echo result correlated to a peer Endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EchoReplyView {
    pub(crate) target_endpoint_id: EndpointId,
    pub(crate) payload: String,
}

impl EchoReplyView {
    /// Creates a bounded Echo response.
    ///
    /// # Errors
    /// Returns invalid input when the payload exceeds 4,096 bytes.
    pub fn new(target_endpoint_id: EndpointId, payload: &str) -> Result<Self, super::ApiError> {
        if payload.len() > 4_096 {
            Err(super::ApiError::invalid_input())
        } else {
            Ok(Self {
                target_endpoint_id,
                payload: payload.to_owned(),
            })
        }
    }
}

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
    json!({"target_endpoint_id": encode_hex(value.target_endpoint_id.as_bytes()), "payload": value.payload})
}

pub(crate) fn ui_auth_result_value(value: &UiAuthView) -> Value {
    super::snapshot::ui_auth_value(value)
}

fn capability_value(value: CapabilityFlags) -> Value {
    json!({"spaces": value.management.spaces, "control_sync": value.management.control_sync, "private_relay": value.relays.private_relay, "public_relay": value.relays.public_relay, "echo": value.interaction.echo, "events": value.interaction.events})
}
