//! Iroh transport adapter boundary for MA2A.

#![forbid(unsafe_code)]

mod address_lookup;
mod address_record;
mod endpoint;
mod enrollment;
mod protocols;

pub use address_lookup::{AddressLookupClock, AddressLookupStateError, SpaceAddressLookup};
pub use address_record::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator,
    AddressValidationContext, ValidatedAddressRecord,
};
pub use endpoint::{EndpointSecret, InvalidEndpointSecret, NetError, RuntimeEndpoint};
pub use enrollment::EnrollmentCall;
pub use iroh::EndpointAddr;
pub use protocols::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS, ProtocolRole};
