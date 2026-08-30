use ma2a_core::{
    ControlCursorEntryV1, ControlCursorV1, EndpointId, ProtocolError, SpaceAuthorizationView,
    SpaceChain,
};

use crate::{PersistedAddressRecord, PersistedRelayAdvertisement, Repository, StoreError};

/// Complete current signed control state for one verified Space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlSpaceState {
    chain: SpaceChain,
    address_records: Vec<PersistedAddressRecord>,
    relay_advertisements: Vec<PersistedRelayAdvertisement>,
}

impl ControlSpaceState {
    /// Returns the verified contiguous Space chain.
    pub const fn chain(&self) -> &SpaceChain {
        &self.chain
    }

    /// Returns current persisted signed address records.
    pub fn address_records(&self) -> &[PersistedAddressRecord] {
        &self.address_records
    }

    /// Returns current persisted signed relay advertisements.
    pub fn relay_advertisements(&self) -> &[PersistedRelayAdvertisement] {
        &self.relay_advertisements
    }

    /// Builds the bounded receiver high-water cursor for this Space.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] when persisted cursor entries exceed protocol bounds.
    pub fn cursor(&self) -> Result<ControlCursorV1, ProtocolError> {
        ControlCursorV1::new(
            self.chain.space_id(),
            self.chain.latest_generation(),
            self.chain.latest_hash(),
        )
        .with_entries(
            self.address_records
                .iter()
                .map(|record| ControlCursorEntryV1::new(record.endpoint_id(), record.sequence()))
                .collect(),
            self.relay_advertisements
                .iter()
                .map(|advertisement| {
                    ControlCursorEntryV1::new(
                        advertisement.provider_endpoint_id(),
                        advertisement.sequence(),
                    )
                })
                .collect(),
        )
    }

    /// Derives the current immutable authorization view.
    pub fn authorization(&self) -> SpaceAuthorizationView {
        SpaceAuthorizationView::from_chain(&self.chain)
    }
}

impl Repository {
    /// Loads current control state for every Space containing one Endpoint.
    ///
    /// # Errors
    /// Returns [`StoreError`] when verified state cannot be loaded.
    pub fn control_spaces_for(
        &self,
        endpoint_id: EndpointId,
    ) -> Result<Vec<ControlSpaceState>, StoreError> {
        let memberships = self.memberships_for(endpoint_id)?;
        let mut spaces = Vec::with_capacity(memberships.len());
        for space_id in memberships {
            let chain = self
                .load_space_chain(space_id)?
                .ok_or(StoreError::SchemaMismatch {
                    detail: "current membership references a missing Space chain",
                })?;
            spaces.push(ControlSpaceState {
                chain,
                address_records: self.address_records_for_space(space_id)?,
                relay_advertisements: self.relay_advertisements_for_space(space_id)?,
            });
        }
        Ok(spaces)
    }

    /// Loads control state only for Spaces currently shared by both Endpoints.
    ///
    /// # Errors
    /// Returns [`StoreError`] when verified state cannot be loaded.
    pub fn control_spaces_between(
        &self,
        local_endpoint_id: EndpointId,
        remote_endpoint_id: EndpointId,
    ) -> Result<Vec<ControlSpaceState>, StoreError> {
        let local_spaces = self.control_spaces_for(local_endpoint_id)?;
        let mut spaces = Vec::with_capacity(local_spaces.len());
        for state in local_spaces {
            let authorization = state.authorization();
            if !authorization.contains_member(remote_endpoint_id) {
                continue;
            }
            spaces.push(state);
        }
        Ok(spaces)
    }
}
