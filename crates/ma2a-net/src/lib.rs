//! Iroh transport adapter boundary for MA2A.

#![forbid(unsafe_code)]

mod address_data;
mod address_lookup;
mod address_observation;
mod address_record;
mod endpoint;
mod enrollment;
mod metrics;
mod private_relay;
mod protocols;
mod publisher;
mod relay_access;
mod relay_advertisement;
mod relay_config;
mod relay_tls;

pub use address_data::{address_endpoint_data_from_iroh, address_endpoint_data_to_iroh};
pub use address_lookup::{AddressLookupClock, AddressLookupStateError, SpaceAddressLookup};
pub use address_record::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator,
    AddressValidationContext, ValidatedAddressRecord,
};
pub use endpoint::{
    EndpointBindOptions, EndpointSecret, InvalidEndpointSecret, NetError, RuntimeEndpoint,
};
pub use enrollment::EnrollmentCall;
pub use iroh::EndpointAddr;
pub use metrics::{
    AddressCacheOutcome, AddressLookupExclusion, AddressMetrics, AddressMetricsSnapshot,
    AddressPersistenceOutcome, AddressValidationOutcome,
};
pub use private_relay::{PrivateRelayServer, PrivateRelayServerError};
pub use protocols::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS, ProtocolRole, ZERO_SPACE_ALPNS};
pub use publisher::{AddressPublishRequest, AddressPublisher, AddressPublisherError};
pub use relay_access::PrivateRelayAccess;
pub use relay_advertisement::{
    AdvertisementPublicationRequest, AdvertisementValidationContext,
    PrivateRelayAdvertisementPublishError, PrivateRelayAdvertisementPublisher,
    PrivateRelayAdvertisementValidationError, PrivateRelayAdvertisementValidator,
    ValidatedPrivateRelayAdvertisement,
};
pub use relay_config::{
    PrivateRelayProviderConfig, PrivateRelayProviderLocation, PrivateRelayTransport,
    PublicRelayFallbackConfig, RelayConfigError, RuntimeRelayConfiguration,
};
pub use relay_tls::{NativeRelayTlsConfig, RelayTlsError};
