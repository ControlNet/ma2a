use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use iroh_base::TransportAddr;

use crate::ProtocolError;
use crate::address::{AddressEndpointDataV1, MAX_ADDRESS_RECORD_ADDRESSES};
use crate::space_codec::{Decoder, write_array, write_bytes, write_text, write_uint};

pub(super) const MAX_RELAY_URL_LEN: usize = 512;

impl AddressEndpointDataV1 {
    pub(super) fn encode(&self, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
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
                _ => return Err(ProtocolError::INVALID_INPUT),
            }
        }
        Ok(())
    }

    pub(super) fn decode(decoder: &mut Decoder<'_>) -> Result<Self, ProtocolError> {
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
                _ => return Err(ProtocolError::INVALID_INPUT),
            };
            addresses.push(address);
        }
        Self::new(addresses)
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
