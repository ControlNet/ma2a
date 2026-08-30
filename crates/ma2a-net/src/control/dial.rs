use std::future::Future;

use ma2a_core::EndpointId;
use tokio::time::{Duration, sleep, timeout};

use crate::{ControlClient, NetError};

use super::{CONTROL_MAX_ATTEMPTS, CONTROL_ROUND_DEADLINE};

/// Maps an injected uniformly distributed sample to capped full-jitter backoff.
pub fn retry_delay(attempt: usize, sample: u64) -> Duration {
    let cap_ms = 100_u64.saturating_mul(1_u64 << attempt.min(4));
    let range = cap_ms.saturating_add(1);
    let scaled = (u128::from(sample) * u128::from(range)) >> u64::BITS;
    Duration::from_millis(u64::try_from(scaled).unwrap_or(cap_ms))
}

fn random_retry_delay(attempt: usize) -> Result<Duration, NetError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|_| NetError::control_permanent())?;
    Ok(retry_delay(attempt, u64::from_ne_bytes(bytes)))
}

async fn exchange_with_retry<Exchange, ExchangeFuture, Jitter>(
    mut exchange: Exchange,
    mut jitter: Jitter,
) -> Result<Vec<u8>, NetError>
where
    Exchange: FnMut() -> ExchangeFuture,
    ExchangeFuture: Future<Output = Result<Vec<u8>, NetError>>,
    Jitter: FnMut(usize) -> Result<Duration, NetError>,
{
    timeout(CONTROL_ROUND_DEADLINE, async {
        for attempt in 0..CONTROL_MAX_ATTEMPTS {
            match exchange().await {
                Ok(response) => return Ok(response),
                Err(error)
                    if !error.is_transient_control() || attempt + 1 == CONTROL_MAX_ATTEMPTS =>
                {
                    return Err(error);
                }
                Err(_) => sleep(jitter(attempt)?).await,
            }
        }
        Err(NetError::control_transient())
    })
    .await
    .map_err(|_| NetError::control_transient())?
}

/// Actively dials one peer with at most three attempts and one aggregate deadline.
///
/// # Errors
/// Returns [`NetError`] after the bounded attempt or round deadline is exhausted.
pub async fn exchange_control_with_retry(
    client: &ControlClient,
    peer: EndpointId,
    request: &[u8],
) -> Result<Vec<u8>, NetError> {
    exchange_with_retry(
        || client.exchange_endpoint(peer, request),
        random_retry_delay,
    )
    .await
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, future::ready};

    use super::{exchange_with_retry, retry_delay};
    use crate::NetError;

    #[test]
    fn full_jitter_uses_injected_sample_across_the_entire_cap() {
        // Given
        let minimum_sample = 0;
        let maximum_sample = u64::MAX;

        // When
        let minimum = retry_delay(0, minimum_sample);
        let maximum = retry_delay(0, maximum_sample);
        let capped = retry_delay(usize::MAX, maximum_sample);

        // Then
        assert_eq!(minimum, std::time::Duration::ZERO);
        assert_eq!(maximum, std::time::Duration::from_millis(100));
        assert_eq!(capped, std::time::Duration::from_millis(1_600));
    }

    #[tokio::test(start_paused = true)]
    async fn permanent_control_failure_is_not_retried() {
        // Given
        let attempts = Cell::new(0_usize);

        // When
        let result = exchange_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                ready(Err(NetError::control_permanent()))
            },
            |_| Ok(std::time::Duration::ZERO),
        )
        .await;

        // Then
        assert!(result.is_err());
        assert_eq!(attempts.get(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn unreachable_peer_exhausts_three_attempts_without_wall_clock_delay() {
        // Given
        let attempts = Cell::new(0_usize);

        // When
        let result = exchange_with_retry(
            || {
                attempts.set(attempts.get() + 1);
                ready(Err(NetError::control_transient()))
            },
            |_| Ok(std::time::Duration::ZERO),
        )
        .await;

        // Then
        assert!(result.is_err());
        assert_eq!(attempts.get(), 3);
    }
}
