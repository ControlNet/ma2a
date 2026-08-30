//! Deterministic finite retry and cancellation coverage.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use iroh::{Endpoint, endpoint::presets};
use ma2a_net::{
    ConnectionErrorClass, ConnectionManager, ConnectionManagerOptions, DialCancellation, DialClock,
    DialDriver, DialFailure, DialJitter, DialRequest, DialRetryPolicy, EndpointSecret,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
struct FixedClock {
    sleeps: AtomicUsize,
}

impl DialClock for FixedClock {
    fn now_ms(&self) -> u64 {
        1_700_000_000_000
    }

    fn sleep(&self, _delay: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.sleeps.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(()))
    }
}

#[derive(Debug)]
struct MaximumJitter;

impl DialJitter for MaximumJitter {
    fn sample(&self) -> Result<u64, DialFailure> {
        Ok(u64::MAX)
    }
}

#[derive(Debug)]
struct FailingDriver {
    attempts: Arc<AtomicUsize>,
    failure: DialFailure,
}

impl DialDriver for FailingDriver {
    fn dial<'a>(
        &'a self,
        _endpoint: &'a Endpoint,
        _request: &'a DialRequest,
    ) -> Pin<Box<dyn Future<Output = Result<iroh::endpoint::Connection, DialFailure>> + Send + 'a>>
    {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::ready(Err(self.failure.clone())))
    }
}

#[derive(Debug)]
struct PendingDriver {
    attempts: Arc<AtomicUsize>,
}

impl DialDriver for PendingDriver {
    fn dial<'a>(
        &'a self,
        _endpoint: &'a Endpoint,
        _request: &'a DialRequest,
    ) -> Pin<Box<dyn Future<Output = Result<iroh::endpoint::Connection, DialFailure>> + Send + 'a>>
    {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Box::pin(std::future::pending())
    }
}

#[tokio::test]
async fn permanent_authorization_failure_executes_exactly_once() -> TestResult {
    // Given
    let attempts = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(FixedClock {
        sleeps: AtomicUsize::new(0),
    });
    let manager = manager(
        Arc::new(FailingDriver {
            attempts: Arc::clone(&attempts),
            failure: DialFailure::authorization_denied("authorization denied"),
        }),
        Arc::<FixedClock>::clone(&clock),
        DialRetryPolicy::new(5, Duration::from_millis(10), Duration::from_millis(80))?,
    )
    .await?;

    // When
    let result = manager.connect(request(0x41)?).await;

    // Then
    assert_eq!(
        result.err().map(|error| error.class()),
        Some(ConnectionErrorClass::Authorization)
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(clock.sleeps.load(Ordering::SeqCst), 0);
    manager.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn permanent_version_failure_executes_exactly_once() -> TestResult {
    assert_permanent_failure(
        DialFailure::version_mismatch("version mismatch"),
        ConnectionErrorClass::Version,
        0x44,
    )
    .await
}

#[tokio::test]
async fn permanent_revocation_failure_executes_exactly_once() -> TestResult {
    assert_permanent_failure(
        DialFailure::revoked("membership revoked"),
        ConnectionErrorClass::Revocation,
        0x45,
    )
    .await
}

#[tokio::test]
async fn transient_failure_stops_at_the_strict_attempt_cap() -> TestResult {
    // Given
    let attempts = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(FixedClock {
        sleeps: AtomicUsize::new(0),
    });
    let manager = manager(
        Arc::new(FailingDriver {
            attempts: Arc::clone(&attempts),
            failure: DialFailure::transient("unreachable"),
        }),
        Arc::<FixedClock>::clone(&clock),
        DialRetryPolicy::new(3, Duration::from_millis(10), Duration::from_millis(20))?,
    )
    .await?;

    // When
    let result = manager.connect(request(0x42)?).await;

    // Then
    assert_eq!(
        result.err().map(|error| error.class()),
        Some(ConnectionErrorClass::Transient)
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(clock.sleeps.load(Ordering::SeqCst), 2);
    manager.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn cancellation_interrupts_an_in_flight_attempt_without_retry() -> TestResult {
    // Given
    let attempts = Arc::new(AtomicUsize::new(0));
    let cancellation = DialCancellation::new();
    let manager = manager(
        Arc::new(PendingDriver {
            attempts: Arc::clone(&attempts),
        }),
        Arc::new(FixedClock {
            sleeps: AtomicUsize::new(0),
        }),
        DialRetryPolicy::new(3, Duration::from_millis(10), Duration::from_millis(20))?,
    )
    .await?;
    let task = tokio::spawn({
        let manager = manager.clone();
        let request = request(0x43)?.with_cancellation(cancellation.clone());
        async move { manager.connect(request).await }
    });
    while attempts.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }

    // When
    cancellation.cancel();
    let result = task.await?;

    // Then
    assert_eq!(
        result.err().map(|error| error.class()),
        Some(ConnectionErrorClass::Cancelled)
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    manager.shutdown().await;
    Ok(())
}

async fn manager(
    driver: Arc<dyn DialDriver>,
    clock: Arc<dyn DialClock>,
    retry: DialRetryPolicy,
) -> Result<ConnectionManager, Box<dyn std::error::Error + Send + Sync>> {
    let endpoint = Endpoint::builder(presets::Minimal).bind().await?;
    Ok(ConnectionManager::with_options(
        endpoint,
        ConnectionManagerOptions::new(driver, clock, Arc::new(MaximumJitter)).with_retry(retry),
    ))
}

async fn assert_permanent_failure(
    failure: DialFailure,
    expected: ConnectionErrorClass,
    seed: u8,
) -> TestResult {
    let attempts = Arc::new(AtomicUsize::new(0));
    let clock = Arc::new(FixedClock {
        sleeps: AtomicUsize::new(0),
    });
    let manager = manager(
        Arc::new(FailingDriver {
            attempts: Arc::clone(&attempts),
            failure,
        }),
        Arc::<FixedClock>::clone(&clock),
        DialRetryPolicy::new(5, Duration::from_millis(10), Duration::from_millis(80))?,
    )
    .await?;

    let result = manager.connect(request(seed)?).await;

    assert_eq!(result.err().map(|error| error.class()), Some(expected));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(clock.sleeps.load(Ordering::SeqCst), 0);
    manager.shutdown().await;
    Ok(())
}

fn request(seed: u8) -> Result<DialRequest, Box<dyn std::error::Error + Send + Sync>> {
    Ok(DialRequest::new(
        EndpointSecret::parse(&[seed; 32])?.endpoint_id(),
        b"ma2a/test/1".to_vec(),
    ))
}
