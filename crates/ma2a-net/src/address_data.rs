use iroh::address_lookup::{EndpointData, UserData};
use iroh_base::TransportAddr;
use ma2a_core::{
    AddressEndpointDataV1, MAX_ADDRESS_RECORD_ADDRESSES, MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN,
    MAX_ADDRESS_RECORD_USER_DATA_LEN, ProtocolError,
};

const MAX_RELAY_URL_LEN: usize = 512;

/// Converts complete Iroh endpoint data into the bounded signed representation.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] when the Iroh data exceeds protocol bounds.
pub fn address_endpoint_data_from_iroh(
    data: &EndpointData,
) -> Result<AddressEndpointDataV1, ProtocolError> {
    let mut addresses = Vec::new();
    for address in data.addrs() {
        if addresses.len() == MAX_ADDRESS_RECORD_ADDRESSES {
            return Err(ProtocolError::INVALID_INPUT);
        }
        match address {
            TransportAddr::Relay(url) if url.as_str().len() > MAX_RELAY_URL_LEN => {
                return Err(ProtocolError::INVALID_INPUT);
            }
            TransportAddr::Ip(socket) if socket.port() == 0 => {
                return Err(ProtocolError::INVALID_INPUT);
            }
            TransportAddr::Custom(custom)
                if custom.data().len() > MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN =>
            {
                return Err(ProtocolError::INVALID_INPUT);
            }
            TransportAddr::Relay(_) | TransportAddr::Ip(_) | TransportAddr::Custom(_) => {}
            _ => return Err(ProtocolError::INVALID_INPUT),
        }
        addresses.push(address.clone());
    }
    let user_data = data.user_data().map(AsRef::<str>::as_ref);
    if user_data.is_some_and(|value| value.len() > MAX_ADDRESS_RECORD_USER_DATA_LEN) {
        return Err(ProtocolError::INVALID_INPUT);
    }
    AddressEndpointDataV1::from_parts(addresses, user_data.map(str::to_owned))
}

/// Reconstructs complete Iroh endpoint data from the bounded signed representation.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] if user data cannot satisfy pinned Iroh bounds.
pub fn address_endpoint_data_to_iroh(
    data: &AddressEndpointDataV1,
) -> Result<EndpointData, ProtocolError> {
    let mut endpoint_data = EndpointData::new(data.addresses().to_vec());
    endpoint_data.set_user_data(
        data.user_data()
            .map(str::to_owned)
            .map(UserData::try_from)
            .transpose()
            .map_err(|_| ProtocolError::INVALID_INPUT)?,
    );
    Ok(endpoint_data)
}
