use std::time::{SystemTime, UNIX_EPOCH};

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
