//! Iroh transport adapter boundary for MA2A.

#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

mod address_data;
mod address_lookup;
mod address_observation;
mod address_record;
mod connection_manager;
mod connection_options;
mod connection_state;
mod control;
mod dial;
mod echo_protocol;
mod endpoint;
mod enrollment;
mod inbound;
mod metrics;
mod peer_limit;
mod private_relay;
mod protocols;
mod publisher;
mod reconfigure;
mod relay_access;
mod relay_advertisement;
mod relay_config;
mod relay_map;
mod relay_tls;
#[cfg(windows)]
mod relay_tls_windows;
mod service_protocol;
mod telemetry;

pub use address_data::{address_endpoint_data_from_iroh, address_endpoint_data_to_iroh};
pub use address_lookup::{AddressLookupClock, AddressLookupStateError, SpaceAddressLookup};
pub use address_record::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator,
    AddressValidationContext,
};
pub use connection_manager::ConnectionManager;
pub use connection_options::ConnectionManagerOptions;
pub use connection_state::{IrohHomeRelayObservation, IrohRelayObservation};
pub use control::{
    CONTROL_ALPN, CONTROL_DIAL_CONCURRENCY, CONTROL_GLOBAL_LIMIT, CONTROL_MAX_ATTEMPTS,
    CONTROL_PER_PEER_LIMIT, CONTROL_ROUND_DEADLINE, ControlAuthorizedCall, ControlCall,
    ControlClient, ControlMetrics, ControlMetricsSnapshot, ControlRejection, ControlResponder,
    exchange_control_with_retry, is_unseen, retry_delay, select_peer_window,
};
pub use dial::{
    DialCancellation, DialClock, DialDriver, DialFailure, DialJitter, DialPolicyError, DialRequest,
    DialRetryPolicy,
};
pub use echo_protocol::{
    ECHO_ALPN, ECHO_DEADLINE, ECHO_GLOBAL_LIMIT, ECHO_PER_PEER_LIMIT, EchoAuthorizedCall, EchoCall,
    EchoClient, EchoMetrics, EchoMetricsSnapshot, EchoServiceResponse,
};
pub use endpoint::{
    EndpointBindOptions, EndpointSecret, InvalidEndpointSecret, NetError, RuntimeEndpoint,
};
pub use enrollment::EnrollmentCall;
pub use inbound::PEER_CONNECTION_LIMIT;
pub use iroh::{
    EndpointAddr, RelayUrl,
    address_lookup::UserData,
    endpoint::{RecvStream, SendStream},
};
pub use ma2a_store::ValidatedAddressRecord;
pub use metrics::{
    AddressCacheOutcome, AddressLookupExclusion, AddressMetrics, AddressMetricsSnapshot,
    AddressPersistenceOutcome, AddressValidationOutcome,
};
pub use private_relay::{PrivateRelayServer, PrivateRelayServerError};
pub use protocols::{ENROLLMENT_ALPN, NORMAL_PROTOCOL_ALPNS, ProtocolRole, ZERO_SPACE_ALPNS};
pub use publisher::{AddressPublishRequest, AddressPublisher, AddressPublisherError};
pub use reconfigure::{RelayReconfigureError, RelayReconfigureOutcome};
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
pub use relay_map::{LocalIrohRelayMap, PrivateRelayCandidate};
pub use relay_tls::{LoadedRelayTls, NativeRelayTlsConfig, RelayTlsError};
pub use service_protocol::{
    AGENT_ALPN, EXEC_ALPN, FILES_ALPN, PROCESS_ALPN, PROXY_ALPN, SERVICE_ALPN, SHELL_ALPN,
    STREAMING_SERVICES, ServiceCall, ServiceClient, ServiceLimits, ServiceMetrics,
    ServiceMetricsSnapshot, ServicePermit, ServiceStream, TRANSFER_ALPN, service_alpn,
};
pub(crate) use telemetry::ConnectionObservationContext;
pub use telemetry::{
    ConnectionErrorClass, ConnectionErrorObservation, ConnectionObservation, ConnectionPathState,
    ConnectionTelemetry,
};
