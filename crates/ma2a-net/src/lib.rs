//! Iroh transport adapter boundary for MA2A.

#![forbid(unsafe_code)]

mod address_data;
mod address_lookup;
mod address_observation;
mod address_record;
mod endpoint;
mod enrollment;
mod metrics;
mod protocols;
mod publisher;

pub use address_data::{address_endpoint_data_from_iroh, address_endpoint_data_to_iroh};
pub use address_lookup::{AddressLookupClock, AddressLookupStateError, SpaceAddressLookup};
pub use address_record::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator,
    AddressValidationContext, ValidatedAddressRecord,
};
pub use endpoint::{EndpointSecret, InvalidEndpointSecret, NetError, RuntimeEndpoint};
pub use enrollment::EnrollmentCall;
pub use iroh::EndpointAddr;
pub use metrics::{
    AddressCacheOutcome, AddressLookupExclusion, AddressMetrics, AddressMetricsSnapshot,
    AddressPersistenceOutcome, AddressValidationOutcome,
};
pub use protocols::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS, ProtocolRole};
pub use publisher::{AddressPublishRequest, AddressPublisher, AddressPublisherError};
