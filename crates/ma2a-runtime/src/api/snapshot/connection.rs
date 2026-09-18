//! Observational connection and relay-candidate projections.

use ma2a_core::{EndpointId, SpaceId};

use crate::api::ApiError;

/// Bounded observations Iroh retains for one remote Endpoint, oldest first.
pub const MAX_RETAINED_OBSERVATIONS: usize = 8;

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

/// One bounded relay candidate exposed without broad topology data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayCandidateView {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) relay_kind: String,
    pub(crate) eligible: bool,
    pub(crate) covered_space_ids: Vec<SpaceId>,
}

impl RelayCandidateView {
    /// Creates one relay candidate with the exact Spaces its advertisements cover.
    ///
    /// `eligible` is the home-relay-compatible verdict, which is true exactly
    /// when `covered_space_ids` equals every currently active Space.
    ///
    /// # Errors
    /// Returns invalid input when the relay kind length is outside the bound or
    /// the coverage set exceeds the collection bound.
    pub fn new(
        endpoint_id: EndpointId,
        relay_kind: &str,
        eligible: bool,
        covered_space_ids: Vec<SpaceId>,
    ) -> Result<Self, ApiError> {
        if relay_kind.is_empty()
            || relay_kind.len() > 32
            || covered_space_ids.len() > crate::api::MAX_COLLECTION_ITEMS
        {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                endpoint_id,
                relay_kind: relay_kind.to_owned(),
                eligible,
                covered_space_ids,
            })
        }
    }
}
