use std::{
    error::Error,
    future::pending,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use ma2a_net::{CONTROL_DIAL_CONCURRENCY, EndpointSecret};
use tokio::sync::Semaphore;

use super::{DialQueue, run_before_deadline, spawn_control_dials};
use crate::control_sync::PreparedControlPeer;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

fn prepared_peer(seed: u8) -> TestResultValue<PreparedControlPeer> {
    let peer = EndpointSecret::parse(&[seed; 32])?.endpoint_id();
    Ok(PreparedControlPeer {
        peer,
        request: vec![seed],
        spaces: Vec::new(),
    })
}

#[tokio::test(start_paused = true)]
async fn aggregate_round_deadline_cancels_hanging_dials() -> TestResult {
    // Given
    // When
    let task = tokio::spawn(async {
        let deadline = tokio::time::sleep(ma2a_net::CONTROL_ROUND_DEADLINE);
        tokio::pin!(deadline);
        run_before_deadline(&mut deadline, pending::<()>()).await
    });
    tokio::task::yield_now().await;
    tokio::time::advance(ma2a_net::CONTROL_ROUND_DEADLINE).await;
    let result = task.await?;

    // Then
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn dial_collection_never_exceeds_runtime_concurrency() -> TestResult {
    // Given
    let queue = DialQueue::new(
        (0..=CONTROL_DIAL_CONCURRENCY)
            .map(|offset| prepared_peer(0x50_u8.saturating_add(u8::try_from(offset)?)))
            .collect::<TestResultValue<Vec<_>>>()?,
    );
    let gate = Arc::new(Semaphore::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));

    // When
    let mut queue = queue;
    let mut dials = tokio::task::JoinSet::new();
    spawn_control_dials(&mut queue, &mut dials, {
        let gate = Arc::clone(&gate);
        let active = Arc::clone(&active);
        let maximum = Arc::clone(&maximum);
        let started = Arc::clone(&started);
        move |_, _| {
            let gate = Arc::clone(&gate);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let started = Arc::clone(&started);
            async move {
                started.fetch_add(1, Ordering::SeqCst);
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                gate.acquire()
                    .await
                    .map_err(std::io::Error::other)?
                    .forget();
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<Vec<u8>, std::io::Error>(Vec::new())
            }
        }
    });
    while started.load(Ordering::SeqCst) < CONTROL_DIAL_CONCURRENCY {
        tokio::task::yield_now().await;
    }

    // Then
    assert_eq!(started.load(Ordering::SeqCst), CONTROL_DIAL_CONCURRENCY);
    assert_eq!(maximum.load(Ordering::SeqCst), CONTROL_DIAL_CONCURRENCY);
    gate.add_permits(CONTROL_DIAL_CONCURRENCY + 1);
    let mut completed = 0;
    while let Some(joined) = dials.join_next().await {
        let (peer, _spaces, response) = joined?;
        response?;
        queue.finished(peer, true);
        completed += 1;
        spawn_control_dials(&mut queue, &mut dials, {
            let gate = Arc::clone(&gate);
            move |_, _| {
                let gate = Arc::clone(&gate);
                async move {
                    gate.acquire()
                        .await
                        .map_err(std::io::Error::other)?
                        .forget();
                    Ok::<Vec<u8>, std::io::Error>(Vec::new())
                }
            }
        });
    }
    assert_eq!(completed, CONTROL_DIAL_CONCURRENCY + 1);
    Ok(())
}
