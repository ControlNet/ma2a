use tokio::sync::mpsc;

use crate::{PublicRelayFallbackConfig, control::ControlCall, enrollment::EnrollmentCall};

/// Enrollment routing inputs for a lookup-aware Runtime Endpoint bind.
#[derive(Debug)]
pub struct EndpointBindOptions {
    pub(super) enrollment_calls: mpsc::Sender<EnrollmentCall>,
    pub(super) bind_port: Option<u16>,
    pub(super) relay_map: Option<crate::LocalIrohRelayMap>,
    pub(super) control_calls: Option<mpsc::Sender<ControlCall>>,
    pub(super) control_enabled: bool,
}

impl EndpointBindOptions {
    /// Creates enrollment routing inputs with an optional fixed UDP port.
    pub const fn new(
        enrollment_calls: mpsc::Sender<EnrollmentCall>,
        bind_port: Option<u16>,
    ) -> Self {
        Self {
            enrollment_calls,
            bind_port,
            relay_map: None,
            control_calls: None,
            control_enabled: false,
        }
    }

    /// Enables explicit operator-supplied public relay transport fallback.
    #[must_use]
    pub fn with_public_relay_fallback(
        mut self,
        public_relay_fallback: &PublicRelayFallbackConfig,
    ) -> Self {
        self.relay_map = Some(crate::LocalIrohRelayMap::from_control_spaces(
            &[],
            Some(public_relay_fallback),
            0,
        ));
        self
    }

    /// Supplies the complete safe private and public relay candidate map.
    #[must_use]
    pub fn with_relay_map(mut self, relay_map: crate::LocalIrohRelayMap) -> Self {
        self.relay_map = Some(relay_map);
        self
    }

    /// Registers the existing-member control handler and initial ALPN eligibility.
    #[must_use]
    pub fn with_control(mut self, control_calls: mpsc::Sender<ControlCall>, enabled: bool) -> Self {
        self.control_calls = Some(control_calls);
        self.control_enabled = enabled;
        self
    }
}
