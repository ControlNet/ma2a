use std::{fmt, sync::Arc};

use crate::{
    DialClock, DialDriver, DialJitter, DialRetryPolicy,
    dial::{IrohDialDriver, SystemDialClock, SystemDialJitter},
};

/// Injected dependencies and retry bounds for a connection manager.
#[derive(Clone)]
pub struct ConnectionManagerOptions {
    pub(crate) driver: Arc<dyn DialDriver>,
    pub(crate) clock: Arc<dyn DialClock>,
    pub(crate) jitter: Arc<dyn DialJitter>,
    pub(crate) retry: DialRetryPolicy,
}

impl ConnectionManagerOptions {
    /// Creates options with injected attempt, time, and jitter providers.
    pub fn new(
        driver: Arc<dyn DialDriver>,
        clock: Arc<dyn DialClock>,
        jitter: Arc<dyn DialJitter>,
    ) -> Self {
        Self {
            driver,
            clock,
            jitter,
            retry: DialRetryPolicy::default(),
        }
    }

    /// Replaces the finite retry policy.
    #[must_use]
    pub const fn with_retry(mut self, retry: DialRetryPolicy) -> Self {
        self.retry = retry;
        self
    }
}

impl fmt::Debug for ConnectionManagerOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConnectionManagerOptions")
            .field("driver", &self.driver)
            .field("clock", &self.clock)
            .field("jitter", &self.jitter)
            .field("retry", &self.retry)
            .finish()
    }
}

impl Default for ConnectionManagerOptions {
    fn default() -> Self {
        Self::new(
            Arc::new(IrohDialDriver),
            Arc::new(SystemDialClock),
            Arc::new(SystemDialJitter),
        )
    }
}
