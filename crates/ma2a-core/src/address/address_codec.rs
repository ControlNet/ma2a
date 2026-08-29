use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use iroh_base::{CustomAddr, TransportAddr};

use crate::ProtocolError;
use crate::address::{
    AddressEndpointDataV1, MAX_ADDRESS_RECORD_ADDRESSES, MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN,
    MAX_ADDRESS_RECORD_USER_DATA_LEN,
};
use crate::space_codec::{Decoder, write_array, write_bytes, write_null, write_text, write_uint};

pub(super) const MAX_RELAY_URL_LEN: usize = 512;

impl AddressEndpointDataV1 {
    pub(super) fn encode(&self, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
        write_array(output, 2)?;
        write_array(output, self.addresses.len())?;
        for address in &self.addresses {
            write_array(output, 2)?;
            match address {
                TransportAddr::Relay(url) => {
                    write_uint(output, 0);
                    write_text(output, url.as_str())?;
                }
                TransportAddr::Ip(SocketAddr::V4(socket)) => {
                    write_uint(output, 1);
                    let mut bytes = [0u8; 6];
                    bytes[..4].copy_from_slice(&socket.ip().octets());
                    bytes[4..].copy_from_slice(&socket.port().to_be_bytes());
                    write_bytes(output, &bytes)?;
                }
                TransportAddr::Ip(SocketAddr::V6(socket)) => {
                    write_uint(output, 2);
                    let mut bytes = [0u8; 18];
                    bytes[..16].copy_from_slice(&socket.ip().octets());
                    bytes[16..].copy_from_slice(&socket.port().to_be_bytes());
                    write_bytes(output, &bytes)?;
                }
                TransportAddr::Custom(custom) => {
                    write_uint(output, 3);
                    write_array(output, 2)?;
                    write_uint(output, custom.id());
                    write_bytes(output, custom.data())?;
                }
                _ => return Err(ProtocolError::INVALID_INPUT),
            }
        }
        match &self.user_data {
            Some(user_data) => write_text(output, user_data)?,
            None => write_null(output),
        }
        Ok(())
    }

    pub(super) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
        if decoder.array(2)? != 2 {
            return Err(ProtocolError::INVALID_INPUT);
        }
        let count = decoder.array(MAX_ADDRESS_RECORD_ADDRESSES)?;
        let mut addresses = Vec::new();
        addresses
            .try_reserve(count)
            .map_err(|_| ProtocolError::INTERNAL)?;
        for _ in 0..count {
            if decoder.array(2)? != 2 {
                return Err(ProtocolError::INVALID_INPUT);
            }
            let kind = decoder.uint()?;
            let address = match kind {
                0 => TransportAddr::Relay(
                    decoder
                        .text(MAX_RELAY_URL_LEN)?
                        .parse()
                        .map_err(|_| ProtocolError::INVALID_INPUT)?,
                ),
                1 => TransportAddr::Ip(decode_ipv4(decoder.bytes(6)?)?),
                2 => TransportAddr::Ip(decode_ipv6(decoder.bytes(18)?)?),
                3 => {
                    if decoder.array(2)? != 2 {
                        return Err(ProtocolError::INVALID_INPUT);
                    }
                    let id = decoder.uint()?;
                    let data = decoder.bytes(MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN)?;
                    TransportAddr::Custom(CustomAddr::from_parts(id, data))
                }
                _ => return Err(ProtocolError::INVALID_INPUT),
            };
            addresses.push(address);
        }
        let user_data = decoder
            .optional_text(MAX_ADDRESS_RECORD_USER_DATA_LEN)?
            .map(str::to_owned);
        Self::from_parts(addresses, user_data)
    }
}

fn decode_ipv4(bytes: &[u8]) -> Result<SocketAddr, ProtocolError> {
    let bytes = <&[u8; 6]>::try_from(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::new(bytes[0], bytes[1], bytes[2], bytes[3])),
        u16::from_be_bytes([bytes[4], bytes[5]]),
    ))
}

fn decode_ipv6(bytes: &[u8]) -> Result<SocketAddr, ProtocolError> {
    let bytes = <&[u8; 18]>::try_from(bytes).map_err(|_| ProtocolError::INVALID_INPUT)?;
    let octets = <[u8; 16]>::try_from(&bytes[..16]).map_err(|_| ProtocolError::INVALID_INPUT)?;
    Ok(SocketAddr::new(
        IpAddr::V6(Ipv6Addr::from(octets)),
        u16::from_be_bytes([bytes[16], bytes[17]]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_rejects_oversized_custom_before_copy() {
        // Given
        let mut bytes = vec![0x82, 0x81, 0x82, 0x03, 0x82, 0x01, 0x59, 0x04, 0x01];
        bytes.extend(std::iter::repeat_n(
            0x5a,
            MAX_ADDRESS_RECORD_CUSTOM_DATA_LEN + 1,
        ));
        bytes.push(0xf6);
        let mut decoder = Decoder::new(&bytes);

        // When
        let result = AddressEndpointDataV1::decode(&mut decoder);

        // Then
        assert_eq!(result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn decoder_rejects_invalid_or_oversized_user_data() {
        // Given
        let invalid_utf8 = [0x82, 0x80, 0x61, 0xff];
        let mut oversized = vec![0x82, 0x80, 0x78, 0xf6];
        oversized.extend(std::iter::repeat_n(
            b'a',
            MAX_ADDRESS_RECORD_USER_DATA_LEN + 1,
        ));

        // When
        let invalid_result = AddressEndpointDataV1::decode(&mut Decoder::new(&invalid_utf8));
        let oversized_result = AddressEndpointDataV1::decode(&mut Decoder::new(&oversized));

        // Then
        assert_eq!(invalid_result, Err(ProtocolError::INVALID_INPUT));
        assert_eq!(oversized_result, Err(ProtocolError::INVALID_INPUT));
    }

    #[test]
    fn decoder_rejects_unknown_nonminimal_and_extra_shapes() {
        // Given
        let unknown = [0x82, 0x81, 0x82, 0x04, 0xf6, 0xf6];
        let nonminimal = [0x82, 0x81, 0x82, 0x18, 0x03, 0x82, 0x01, 0x40, 0xf6];
        let extra = [0x83, 0x80, 0xf6, 0xf6];

        // When
        let unknown_result = AddressEndpointDataV1::decode(&mut Decoder::new(&unknown));
        let nonminimal_result = AddressEndpointDataV1::decode(&mut Decoder::new(&nonminimal));
        let extra_result = AddressEndpointDataV1::decode(&mut Decoder::new(&extra));

        // Then
        assert_eq!(unknown_result, Err(ProtocolError::INVALID_INPUT));
        assert_eq!(nonminimal_result, Err(ProtocolError::INVALID_INPUT));
        assert_eq!(extra_result, Err(ProtocolError::INVALID_INPUT));
    }
}
