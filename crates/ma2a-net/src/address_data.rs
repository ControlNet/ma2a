use iroh::address_lookup::{EndpointData, UserData};
use ma2a_core::{AddressEndpointDataV1, ProtocolError};

/// Converts complete Iroh endpoint data into the bounded signed representation.
///
/// # Errors
/// Returns [`ProtocolError::INVALID_INPUT`] when the Iroh data exceeds protocol bounds.
pub fn address_endpoint_data_from_iroh(
    data: &EndpointData,
) -> Result<AddressEndpointDataV1, ProtocolError> {
    AddressEndpointDataV1::from_parts(
        data.addrs().cloned().collect(),
        data.user_data().map(ToString::to_string),
    )
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
