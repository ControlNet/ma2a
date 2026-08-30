use std::{error::Error, fmt};

use crate::address_observation::AddressObservationWaitError;

/// Invalid protected Endpoint key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidEndpointSecret {
    found: usize,
}

impl InvalidEndpointSecret {
    pub(super) const fn new(found: usize) -> Self {
        Self { found }
    }

    /// Returns the invalid byte length without exposing key material.
    pub const fn found_length(self) -> usize {
        self.found
    }
}

impl fmt::Display for InvalidEndpointSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "protected Endpoint key has invalid length {}; expected 32 bytes",
            self.found
        )
    }
}

impl Error for InvalidEndpointSecret {}

/// Failures while binding or shutting down the Iroh Endpoint.
#[derive(Debug)]
pub struct NetError(NetErrorKind);

#[derive(Debug)]
enum NetErrorKind {
    Bind(iroh::endpoint::BindError),
    Observation(AddressObservationWaitError),
    Shutdown,
    Enrollment,
    Control(ControlFailure),
    Reconfigure(crate::RelayReconfigureError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlFailure {
    Transient,
    Permanent,
}

impl fmt::Display for NetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            NetErrorKind::Bind(error) => write!(formatter, "Iroh Endpoint bind failed: {error}"),
            NetErrorKind::Observation(error) => {
                write!(formatter, "Iroh Endpoint observation failed: {error}")
            }
            NetErrorKind::Shutdown => formatter.write_str("Iroh Endpoint shutdown task failed"),
            NetErrorKind::Enrollment => formatter.write_str("Iroh enrollment exchange failed"),
            NetErrorKind::Control(_) => formatter.write_str("Iroh control exchange failed"),
            NetErrorKind::Reconfigure(error) => error.fmt(formatter),
        }
    }
}

impl NetError {
    pub(super) const fn bind(error: iroh::endpoint::BindError) -> Self {
        Self(NetErrorKind::Bind(error))
    }

    pub(super) const fn observation(error: AddressObservationWaitError) -> Self {
        Self(NetErrorKind::Observation(error))
    }

    pub(super) const fn shutdown() -> Self {
        Self(NetErrorKind::Shutdown)
    }

    pub(crate) const fn enrollment() -> Self {
        Self(NetErrorKind::Enrollment)
    }

    pub(crate) const fn control_transient() -> Self {
        Self(NetErrorKind::Control(ControlFailure::Transient))
    }

    pub(crate) const fn control_permanent() -> Self {
        Self(NetErrorKind::Control(ControlFailure::Permanent))
    }

    pub(crate) const fn control_from_dial(error: &crate::DialFailure) -> Self {
        match error.class() {
            crate::ConnectionErrorClass::Transient | crate::ConnectionErrorClass::Cancelled => {
                Self::control_transient()
            }
            crate::ConnectionErrorClass::Authorization
            | crate::ConnectionErrorClass::Version
            | crate::ConnectionErrorClass::Revocation
            | crate::ConnectionErrorClass::MalformedInput
            | crate::ConnectionErrorClass::Policy => Self::control_permanent(),
        }
    }

    pub(crate) const fn reconfigure(error: crate::RelayReconfigureError) -> Self {
        Self(NetErrorKind::Reconfigure(error))
    }

    pub(crate) const fn is_transient_control(&self) -> bool {
        matches!(self.0, NetErrorKind::Control(ControlFailure::Transient))
    }
}

impl Error for NetError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.0 {
            NetErrorKind::Bind(error) => Some(error),
            NetErrorKind::Observation(error) => Some(error),
            NetErrorKind::Reconfigure(error) => Some(error),
            NetErrorKind::Shutdown | NetErrorKind::Enrollment | NetErrorKind::Control(_) => None,
        }
    }
}
