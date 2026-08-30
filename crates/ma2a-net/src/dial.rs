use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use iroh::{Endpoint, EndpointAddr, endpoint::Connection};
use ma2a_core::EndpointId;
use tokio::sync::Notify;

use crate::ConnectionErrorClass;

mod retry;

pub(crate) use retry::{
    DialDependencies, IrohDialDriver, SystemDialClock, SystemDialJitter, dial_with_retry,
};

const MAX_ERROR_DETAIL_BYTES: usize = 160;

/// One bounded connection attempt request for an exact Endpoint and ALPN.
#[derive(Clone, Debug)]
pub struct DialRequest {
    target: EndpointId,
    endpoint_addr: Option<EndpointAddr>,
    alpn: Vec<u8>,
    cancellation: DialCancellation,
}

impl DialRequest {
    /// Creates a target-only request resolved by the Endpoint's configured address lookup.
    pub fn new(target: EndpointId, alpn: Vec<u8>) -> Self {
        Self {
            target,
            endpoint_addr: None,
            alpn,
            cancellation: DialCancellation::new(),
        }
    }

    pub(crate) fn from_addr(endpoint_addr: EndpointAddr, alpn: Vec<u8>) -> Self {
        Self {
            target: endpoint_addr.id.into(),
            endpoint_addr: Some(endpoint_addr),
            alpn,
            cancellation: DialCancellation::new(),
        }
    }

    /// Supplies cancellation scoped to this request.
    #[must_use]
    pub fn with_cancellation(mut self, cancellation: DialCancellation) -> Self {
        self.cancellation = cancellation;
        self
    }

    /// Returns the requested remote Endpoint identity.
    pub const fn target(&self) -> EndpointId {
        self.target
    }

    /// Returns the requested ALPN bytes.
    pub fn alpn(&self) -> &[u8] {
        &self.alpn
    }

    fn iroh_target(&self) -> Result<EndpointAddr, DialFailure> {
        self.endpoint_addr.clone().map_or_else(
            || {
                self.target
                    .to_public_key()
                    .map(EndpointAddr::from)
                    .map_err(|_| DialFailure::malformed_input("invalid Endpoint identity"))
            },
            Ok,
        )
    }
}

/// Cooperative cancellation for one dial or reconnect operation.
#[derive(Clone, Debug)]
pub struct DialCancellation(Arc<CancellationState>);

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl DialCancellation {
    /// Creates an active cancellation handle.
    pub fn new() -> Self {
        Self(Arc::new(CancellationState {
            cancelled: AtomicBool::new(false),
            notify: Notify::new(),
        }))
    }

    /// Cancels current and future waits.
    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.notify.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

impl Default for DialCancellation {
    fn default() -> Self {
        Self::new()
    }
}

/// Injected timestamp and delay source for deterministic recovery.
pub trait DialClock: fmt::Debug + Send + Sync + 'static {
    /// Returns Unix time in milliseconds.
    fn now_ms(&self) -> u64;
    /// Waits for the requested retry delay.
    fn sleep(&self, delay: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Injected full-jitter sample source.
pub trait DialJitter: fmt::Debug + Send + Sync + 'static {
    /// Returns one uniformly distributed sample.
    ///
    /// # Errors
    /// Returns [`DialFailure`] when the random source is unavailable.
    fn sample(&self) -> Result<u64, DialFailure>;
}

/// Injectable adapter for one underlying Iroh connection attempt.
pub trait DialDriver: fmt::Debug + Send + Sync + 'static {
    /// Starts exactly one attempt without application-level retry.
    fn dial<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        request: &'a DialRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Connection, DialFailure>> + Send + 'a>>;
}

/// Validated capped exponential retry policy.
#[derive(Clone, Copy, Debug)]
pub struct DialRetryPolicy {
    attempts: usize,
    base: Duration,
    cap: Duration,
}

impl DialRetryPolicy {
    /// Creates a finite retry policy.
    ///
    /// # Errors
    /// Returns [`DialPolicyError`] when attempts are zero, the base delay is zero, or the cap is
    /// below the base delay.
    pub fn new(attempts: usize, base: Duration, cap: Duration) -> Result<Self, DialPolicyError> {
        if attempts == 0 || base.is_zero() || cap < base {
            return Err(DialPolicyError);
        }
        Ok(Self {
            attempts,
            base,
            cap,
        })
    }

    pub(crate) const fn attempts(self) -> usize {
        self.attempts
    }

    pub(crate) const fn single_attempt() -> Self {
        Self {
            attempts: 1,
            base: Duration::from_millis(1),
            cap: Duration::from_millis(1),
        }
    }

    pub(crate) fn delay(self, attempt: usize, sample: u64) -> Duration {
        let exponent = u32::try_from(attempt.min(31)).unwrap_or(31);
        let exponential = self.base.saturating_mul(2_u32.saturating_pow(exponent));
        let cap = exponential.min(self.cap);
        let range = cap.as_millis().saturating_add(1);
        let scaled = (u128::from(sample) * range) >> u64::BITS;
        Duration::from_millis(u64::try_from(scaled).unwrap_or(u64::MAX))
    }
}

impl Default for DialRetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 3,
            base: Duration::from_millis(100),
            cap: Duration::from_millis(1_600),
        }
    }
}

/// Invalid retry bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct DialPolicyError;

impl fmt::Display for DialPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("dial retry policy is invalid")
    }
}

impl Error for DialPolicyError {}

/// Typed, bounded dial failure used to decide retry eligibility.
#[derive(Clone, Debug)]
pub struct DialFailure {
    class: ConnectionErrorClass,
    detail: String,
    attempts: usize,
}

impl DialFailure {
    /// Creates a retryable transport failure.
    pub fn transient(detail: &str) -> Self {
        Self::new(ConnectionErrorClass::Transient, detail)
    }
    /// Creates a permanent authorization denial.
    pub fn authorization_denied(detail: &str) -> Self {
        Self::new(ConnectionErrorClass::Authorization, detail)
    }
    /// Creates a permanent protocol-version or ALPN mismatch.
    pub fn version_mismatch(detail: &str) -> Self {
        Self::new(ConnectionErrorClass::Version, detail)
    }
    /// Creates a permanent revocation failure.
    pub fn revoked(detail: &str) -> Self {
        Self::new(ConnectionErrorClass::Revocation, detail)
    }
    /// Creates a permanent malformed-input failure.
    pub fn malformed_input(detail: &str) -> Self {
        Self::new(ConnectionErrorClass::MalformedInput, detail)
    }
    /// Creates a permanent local policy denial.
    pub fn policy_denied(detail: &str) -> Self {
        Self::new(ConnectionErrorClass::Policy, detail)
    }
    pub(crate) fn cancelled() -> Self {
        Self::new(ConnectionErrorClass::Cancelled, "dial cancelled")
    }
    /// Returns the stable retry and telemetry class.
    pub const fn class(&self) -> ConnectionErrorClass {
        self.class
    }
    /// Returns the number of attempts executed.
    pub const fn attempts(&self) -> usize {
        self.attempts
    }
    pub(crate) fn detail(&self) -> &str {
        &self.detail
    }
    pub(crate) const fn is_transient(&self) -> bool {
        matches!(self.class, ConnectionErrorClass::Transient)
    }
    pub(crate) const fn with_attempts(mut self, attempts: usize) -> Self {
        self.attempts = attempts;
        self
    }
    fn new(class: ConnectionErrorClass, detail: &str) -> Self {
        let mut end = detail.len().min(MAX_ERROR_DETAIL_BYTES);
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        let bounded = detail[..end].to_owned();
        Self {
            class,
            detail: bounded,
            attempts: 1,
        }
    }
}

impl fmt::Display for DialFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl Error for DialFailure {}
