//! Relay projections that keep the two relay ontologies apart.
//!
//! An MA2A Private Relay is a role hosted by an MA2A Endpoint: it has a provider
//! Endpoint identity and Space-scoped advertisements. A public Iroh relay is
//! external transport infrastructure: it has no Endpoint identity, is never a
//! Space member, and carries no Space coverage. They never share a wire type.

use ma2a_core::{EndpointId, SpaceId};

use crate::api::ApiError;

/// One MA2A Private Relay candidate and the Spaces its advertisements cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateRelayCandidateView {
    pub(crate) provider_endpoint_id: EndpointId,
    pub(crate) relay_url: String,
    pub(crate) covered_space_ids: Vec<SpaceId>,
    pub(crate) home_compatible: bool,
}

impl PrivateRelayCandidateView {
    /// Creates one private candidate.
    ///
    /// `home_compatible` is true exactly when `covered_space_ids` equals every
    /// currently active Space, so the verdict and its reason travel together.
    ///
    /// # Errors
    /// Returns invalid input when the URL length or coverage set is out of bounds.
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors the fixed private relay wire fields"
    )]
    pub fn new(
        provider_endpoint_id: EndpointId,
        relay_url: &str,
        covered_space_ids: Vec<SpaceId>,
        home_compatible: bool,
    ) -> Result<Self, ApiError> {
        if relay_url.is_empty() || relay_url.len() > 2_048 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                provider_endpoint_id,
                relay_url: relay_url.to_owned(),
                covered_space_ids,
                home_compatible,
            })
        }
    }
}

/// One explicitly configured public Iroh relay fallback.
///
/// It deliberately has no Endpoint identity and no Space coverage: inventing
/// either to fit a shared UI type would misstate the ontology.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRelayFallbackView {
    pub(crate) relay_url: String,
    pub(crate) enabled: bool,
    pub(crate) observed_connected: bool,
}

impl PublicRelayFallbackView {
    /// Creates one public fallback entry.
    ///
    /// # Errors
    /// Returns invalid input when the URL length is outside the bound.
    pub fn new(relay_url: &str, enabled: bool, observed_connected: bool) -> Result<Self, ApiError> {
        if relay_url.is_empty() || relay_url.len() > 2_048 {
            Err(ApiError::invalid_input())
        } else {
            Ok(Self {
                relay_url: relay_url.to_owned(),
                enabled,
                observed_connected,
            })
        }
    }
}
