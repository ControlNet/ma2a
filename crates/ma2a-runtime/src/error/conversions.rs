use ma2a_net::{InvalidEndpointSecret, NetError};
use ma2a_store::StoreError;

use super::{RuntimeError, RuntimeErrorKind};

impl From<StoreError> for RuntimeError {
    fn from(error: StoreError) -> Self {
        Self::new(RuntimeErrorKind::Store(error))
    }
}

impl From<ma2a_net::AddressPublisherError> for RuntimeError {
    fn from(error: ma2a_net::AddressPublisherError) -> Self {
        match error {
            ma2a_net::AddressPublisherError::Store(error)
            | ma2a_net::AddressPublisherError::Validation(
                ma2a_net::AddressRecordValidationError::Store(error),
            ) => Self::from(error),
            // An observation outside protocol bounds, such as a host briefly holding more
            // addresses than a record carries, passes like a missing one.
            ma2a_net::AddressPublisherError::ObservationUnavailable
            | ma2a_net::AddressPublisherError::ObservationInvalid(_) => {
                Self::new(RuntimeErrorKind::AddressObservationUnavailable)
            }
            ma2a_net::AddressPublisherError::ClockRollback => {
                Self::new(RuntimeErrorKind::ClockBehind)
            }
            _ => Self::new(RuntimeErrorKind::Control),
        }
    }
}

impl From<ma2a_net::PrivateRelayAdvertisementPublishError> for RuntimeError {
    fn from(error: ma2a_net::PrivateRelayAdvertisementPublishError) -> Self {
        match error {
            ma2a_net::PrivateRelayAdvertisementPublishError::Store(error) => Self::from(error),
            ma2a_net::PrivateRelayAdvertisementPublishError::ClockRollback => {
                Self::new(RuntimeErrorKind::ClockBehind)
            }
            _ => Self::new(RuntimeErrorKind::Control),
        }
    }
}

impl From<NetError> for RuntimeError {
    fn from(error: NetError) -> Self {
        Self::new(RuntimeErrorKind::Network(error))
    }
}

impl From<InvalidEndpointSecret> for RuntimeError {
    fn from(error: InvalidEndpointSecret) -> Self {
        Self::new(RuntimeErrorKind::InvalidEndpointKey(error))
    }
}

impl From<tokio::task::JoinError> for RuntimeError {
    fn from(error: tokio::task::JoinError) -> Self {
        Self::new(RuntimeErrorKind::Task(error))
    }
}
