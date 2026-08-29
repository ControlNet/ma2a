use std::collections::BTreeSet;

use iroh_base::TransportAddr;

use crate::ProtocolError;
use crate::address::{
    MAX_ADDRESS_RECORD_ADDRESSES, MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN,
    MAX_ADDRESS_RECORD_USER_DATA_LEN, address_codec,
};

/// Bounded identity-free Iroh endpoint addressing data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressEndpointDataV1 {
    pub(crate) addresses: Vec<TransportAddr>,
    pub(crate) user_data: Option<String>,
}

impl AddressEndpointDataV1 {
    /// Builds endpoint data without application-defined user data.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for duplicate, invalid, or oversized addresses.
    pub fn new(addresses: Vec<TransportAddr>) -> Result<Self, ProtocolError> {
        Self::from_parts(addresses, None)
    }

    /// Builds complete bounded endpoint data while preserving address priority order.
    ///
    /// # Errors
    /// Returns [`ProtocolError::INVALID_INPUT`] for duplicate, invalid, or oversized data.
    pub fn from_parts(
        addresses: Vec<TransportAddr>,
        user_data: Option<String>,
    ) -> Result<Self, ProtocolError> {
        if addresses.len() > MAX_ADDRESS_RECORD_ADDRESSES
            || user_data
                .as_ref()
                .is_some_and(|value| value.len() > MAX_ADDRESS_RECORD_USER_DATA_LEN)
        {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let mut seen = BTreeSet::new();
        for address in &addresses {
            if !seen.insert(address.clone()) {
                return Err(ProtocolError::INVALID_INPUT);
            }
            match address {
                TransportAddr::Relay(url)
                    if url.as_str().len() <= address_codec::MAX_RELAY_URL_LEN => {}
                TransportAddr::Ip(socket) if socket.port() != 0 => {}
                TransportAddr::Custom(custom)
                    if custom.data().len() <= MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN => {}
                TransportAddr::Relay(_) | TransportAddr::Ip(_) | TransportAddr::Custom(_) => {
                    return Err(ProtocolError::INVALID_INPUT);
                }
                _ => return Err(ProtocolError::INVALID_INPUT),
            }
        }
        Ok(Self {
            addresses,
            user_data,
        })
    }

    /// Returns transport addresses in signed priority order.
    pub fn addresses(&self) -> &[TransportAddr] {
        &self.addresses
    }

    /// Returns optional application-defined UTF-8 data.
    pub fn user_data(&self) -> Option<&str> {
        self.user_data.as_deref()
    }
}
