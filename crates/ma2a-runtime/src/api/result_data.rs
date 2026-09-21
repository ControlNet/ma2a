//! Typed local API result payloads.

use ma2a_core::EndpointId;

mod value;

pub(crate) use value::{
    echo_reply_value, handshake_value, private_relay_value, public_relay_value, status_value,
    ui_auth_result_value, ui_status_value,
};

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
/// Web UI running state and credential-free URL owned by the daemon.
pub struct UiStatusView {
    pub(crate) url: Option<String>,
}

impl UiStatusView {
    /// Creates a status view from the actual bound address, or a stopped state.
    pub fn new(address: Option<std::net::SocketAddr>) -> Self {
        Self {
            url: address.map(|address| format!("http://{address}")),
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
    pub(crate) duration_ms: u16,
}

impl EchoReplyView {
    /// Creates a bounded Echo response.
    ///
    /// # Errors
    /// Returns invalid input when the payload exceeds 4,096 bytes or the
    /// duration exceeds the Echo v1 saturation bound of 10,000 milliseconds.
    pub fn new(
        target_endpoint_id: EndpointId,
        payload: &str,
        duration_ms: u16,
    ) -> Result<Self, super::ApiError> {
        if payload.len() > 4_096 || duration_ms > 10_000 {
            Err(super::ApiError::invalid_input())
        } else {
            Ok(Self {
                target_endpoint_id,
                payload: payload.to_owned(),
                duration_ms,
            })
        }
    }
}
