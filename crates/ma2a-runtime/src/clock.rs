use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{RuntimeError, error::RuntimeErrorKind};

/// Authoritative owner-side time source used for security decisions.
pub trait RuntimeClock: std::fmt::Debug + Send + Sync + 'static {
    /// Returns Unix time in milliseconds.
    ///
    /// # Errors
    /// Returns an error when the clock cannot represent the current instant.
    fn now_ms(&self) -> Result<i64, RuntimeError>;
}

#[derive(Debug)]
pub(crate) struct SystemClock;

impl RuntimeClock for SystemClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?
            .as_millis();
        i64::try_from(millis).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeAddressLookupClock {
    clock: Arc<dyn RuntimeClock>,
}

impl RuntimeAddressLookupClock {
    pub(crate) const fn new(clock: Arc<dyn RuntimeClock>) -> Self {
        Self { clock }
    }
}

impl ma2a_net::AddressLookupClock for RuntimeAddressLookupClock {
    fn now_ms(&self) -> u64 {
        self.clock
            .now_ms()
            .ok()
            .and_then(|value| u64::try_from(value).ok())
            .map_or(0, |value| value)
    }
}
