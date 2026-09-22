//! Observational connection and relay-candidate projections.

use ma2a_core::EndpointId;

use crate::api::ApiError;

/// Bounded observations Iroh retains for one remote Endpoint, oldest first.
pub(crate) const MAX_RETAINED_OBSERVATIONS: usize = 8;

/// One retained observation, exactly as Iroh reported it at that moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionObservationView {
    pub(crate) observed_at_ms: u64,
    pub(crate) path: &'static str,
    pub(crate) rtt_ms: Option<u64>,
    pub(crate) error_class: &'static str,
}

impl ConnectionObservationView {
    /// Creates one retained observation without any authorization diagnostic.
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors the fixed observation wire fields"
    )]
    pub const fn new(
        observed_at_ms: u64,
        path: &'static str,
        rtt_ms: Option<u64>,
        error_class: &'static str,
    ) -> Self {
        Self {
            observed_at_ms,
            path,
            rtt_ms,
            error_class,
        }
    }
}

/// Latest bounded observational connection state for one remote Endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionView {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) state: &'static str,
    pub(crate) path: &'static str,
    pub(crate) rtt_ms: Option<u64>,
    pub(crate) observations: Vec<ConnectionObservationView>,
}

impl ConnectionView {
    /// Creates one per-peer connection projection with its retained history.
    ///
    /// # Errors
    /// Returns invalid input when more observations are supplied than the
    /// telemetry cache is allowed to retain.
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors the fixed connection wire fields"
    )]
    pub fn new(
        endpoint_id: EndpointId,
        state: &'static str,
        path: &'static str,
        rtt_ms: Option<u64>,
        observations: Vec<ConnectionObservationView>,
    ) -> Result<Self, ApiError> {
        if observations.len() > MAX_RETAINED_OBSERVATIONS {
            return Err(ApiError::invalid_input());
        }
        Ok(Self {
            endpoint_id,
            state,
            path,
            rtt_ms,
            observations,
        })
    }
}
