use std::{
    future::Future,
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{Endpoint, endpoint::Connection};

use super::{
    DialCancellation, DialClock, DialDriver, DialFailure, DialJitter, DialRequest, DialRetryPolicy,
};

#[derive(Debug)]
pub(crate) struct SystemDialClock;

impl DialClock for SystemDialClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            .unwrap_or(u64::MAX)
    }

    fn sleep(&self, delay: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(delay))
    }
}

#[derive(Debug)]
pub(crate) struct SystemDialJitter;

impl DialJitter for SystemDialJitter {
    fn sample(&self) -> Result<u64, DialFailure> {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes)
            .map_err(|_| DialFailure::policy_denied("random source unavailable"))?;
        Ok(u64::from_ne_bytes(bytes))
    }
}

#[derive(Debug)]
pub(crate) struct IrohDialDriver;

impl DialDriver for IrohDialDriver {
    fn dial<'a>(
        &'a self,
        endpoint: &'a Endpoint,
        request: &'a DialRequest,
    ) -> Pin<Box<dyn Future<Output = Result<Connection, DialFailure>> + Send + 'a>> {
        Box::pin(async move {
            let target = request.iroh_target()?;
            endpoint
                .connect(target, request.alpn())
                .await
                .map_err(classify_iroh_error)
        })
    }
}

pub(crate) struct DialDependencies<'a> {
    pub(crate) driver: &'a dyn DialDriver,
    pub(crate) clock: &'a dyn DialClock,
    pub(crate) jitter: &'a dyn DialJitter,
    pub(crate) retry: DialRetryPolicy,
    pub(crate) manager_cancellation: &'a DialCancellation,
}

pub(crate) async fn dial_with_retry(
    endpoint: &Endpoint,
    request: &DialRequest,
    dependencies: DialDependencies<'_>,
) -> Result<Connection, DialFailure> {
    for attempt in 0..dependencies.retry.attempts() {
        let attempt_number = attempt + 1;
        let result = tokio::select! {
            biased;
            () = dependencies.manager_cancellation.cancelled() => Err(DialFailure::cancelled()),
            () = request.cancellation.cancelled() => Err(DialFailure::cancelled()),
            result = dependencies.driver.dial(endpoint, request) => result,
        };
        match result {
            Ok(connection) => return Ok(connection),
            Err(error)
                if !error.is_transient() || attempt_number == dependencies.retry.attempts() =>
            {
                return Err(error.with_attempts(attempt_number));
            }
            Err(_) => {
                let delay = dependencies
                    .retry
                    .delay(attempt, dependencies.jitter.sample()?);
                tokio::select! {
                    biased;
                    () = dependencies.manager_cancellation.cancelled() => return Err(DialFailure::cancelled().with_attempts(attempt_number)),
                    () = request.cancellation.cancelled() => return Err(DialFailure::cancelled().with_attempts(attempt_number)),
                    () = dependencies.clock.sleep(delay) => {}
                }
            }
        }
    }
    Err(DialFailure::transient("attempt cap exhausted"))
}

fn classify_iroh_error(error: iroh::endpoint::ConnectError) -> DialFailure {
    use iroh::endpoint::{ConnectError, ConnectWithOptsError, ConnectingError};
    let detail = error.to_string();
    match error {
        ConnectError::Connect { source, .. } => match source {
            ConnectWithOptsError::SelfConnect { .. } | ConnectWithOptsError::InvalidAlpn { .. } => {
                DialFailure::malformed_input(&detail)
            }
            ConnectWithOptsError::LocallyRejected { .. } => DialFailure::policy_denied(&detail),
            ConnectWithOptsError::EndpointClosed { .. }
            | ConnectWithOptsError::NoAddress { .. }
            | ConnectWithOptsError::Noq { .. }
            | ConnectWithOptsError::InternalConsistencyError { .. } => {
                DialFailure::transient(&detail)
            }
            _ => DialFailure::transient(&detail),
        },
        ConnectError::Connecting {
            source:
                ConnectingError::HandshakeFailure { .. } | ConnectingError::LocallyRejected { .. },
            ..
        } => DialFailure::version_mismatch(&detail),
        _ => DialFailure::transient(&detail),
    }
}
