//! Peer rejection is not a local Store/identity/task integrity failure.
use crate::RuntimeError;
use ma2a_net::ControlRejection;
use std::{error::Error, fmt};

#[derive(Debug)]
pub(crate) enum ControlFailure {
    Rejected(ControlRejection),
    Runtime(RuntimeError),
}

impl ControlFailure {
    pub(crate) const fn rejection(&self) -> ControlRejection {
        match self {
            Self::Rejected(rejection) => *rejection,
            Self::Runtime(_) => ControlRejection::Unavailable,
        }
    }
}
impl From<ControlRejection> for ControlFailure {
    fn from(error: ControlRejection) -> Self {
        Self::Rejected(error)
    }
}
impl From<RuntimeError> for ControlFailure {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}
impl From<ma2a_store::StoreError> for ControlFailure {
    fn from(error: ma2a_store::StoreError) -> Self {
        Self::Runtime(error.into())
    }
}
impl fmt::Display for ControlFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected(_) => formatter.write_str("control exchange rejected or unavailable"),
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}
impl Error for ControlFailure {}
