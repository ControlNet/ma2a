//! The lifecycle plane has to answer exactly when the business plane cannot.
//!
//! These are the situations a caller is in when it most needs to ask whether a
//! daemon is alive: its Runtime is stuck behind work that will not finish, or
//! every connection slot is held by callers already stuck behind it. A probe
//! that queues behind that work answers nothing, and an answer that never comes
//! reads to the caller as an absent daemon, which is how a second Runtime gets
//! started beside a wedged first.

use std::{sync::Arc, time::Duration};

use ma2a_store::StoreConfig;
use tokio::sync::{Semaphore, oneshot};

use crate::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{
        BUSINESS_CONNECTION_LIMIT, IpcPaths, LIFECYCLE_DEADLINE, LocalApiClient, LocalApiServer,
    },
    web::{SystemClock, WebAuthConfig},
};

use super::{LiveServer, TempState, TestResult};

/// A server whose business plane is held still, and the client that will ask it.
struct Wedged {
    _state: TempState,
    // The Runtime has to outlive the server: dropping it stops the actor, which
    // is a different situation from the one under test.
    _runtime: Runtime,
    live: LiveServer,
    client: LocalApiClient,
    slots: Arc<Semaphore>,
}

async fn wedged(name: &str) -> Result<Wedged, Box<dyn std::error::Error + Send + Sync>> {
    let state = TempState::new_named(name)?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
    let control = CurrentUserRuntime::open_at(
        &state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let paths = IpcPaths::new(&state.0)?;
    let server =
        LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
    let slots = server.business_slots();
    // Holding this is exactly what a Runtime part-way through a long mutation
    // does, without needing one that takes an unpredictable amount of time.
    let lock = server.business_lock();
    let (acquired, taken) = oneshot::channel();
    let _holder = tokio::spawn(async move {
        let _held = lock.lock().await;
        let _signalled = acquired.send(());
        // Never released: this task is what stands in for work that never ends.
        std::future::pending::<()>().await;
    });
    // Proceeding only once the lock is genuinely held, rather than after a delay
    // that is probably long enough.
    taken.await?;
    Ok(Wedged {
        _state: state,
        _runtime: runtime,
        live: LiveServer::spawn(server),
        client: LocalApiClient::new(paths),
        slots,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lifecycle_call_is_answered_while_the_business_plane_is_blocked() -> TestResult {
    // Given
    let wedged = wedged("blocked-plane").await?;
    let stalled = tokio::spawn({
        let client = wedged.client.clone();
        async move { client.probe().await }
    });

    // When
    let report = wedged.client.ping().await?;

    // Then
    assert_eq!(report.operation(), "pong");
    assert_eq!(
        report.api_version(),
        u64::from(crate::api::LOCAL_API_VERSION)
    );
    assert!(
        !stalled.is_finished(),
        "the business command must still be waiting"
    );
    stalled.abort();
    let _exit = wedged.live.cancel().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lifecycle_call_is_answered_when_every_business_slot_is_taken() -> TestResult {
    // Given
    let wedged = wedged("saturated-plane").await?;
    let stalled = (0..BUSINESS_CONNECTION_LIMIT)
        .map(|_| {
            let client = wedged.client.clone();
            tokio::spawn(async move { client.probe().await })
        })
        .collect::<Vec<_>>();
    // Waiting for the slots themselves, rather than for a duration that might be
    // enough: the guarantee only applies once the pool is genuinely exhausted.
    tokio::time::timeout(Duration::from_secs(30), async {
        while wedged.slots.available_permits() > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await?;

    // When
    let answered = tokio::time::timeout(LIFECYCLE_DEADLINE, wedged.client.ping()).await?;

    // Then
    assert_eq!(answered?.operation(), "pong");
    for task in stalled {
        task.abort();
    }
    let _exit = wedged.live.cancel().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lifecycle_stop_is_accepted_while_the_business_plane_is_blocked() -> TestResult {
    // Given
    let wedged = wedged("stop-while-blocked").await?;
    let stalled = tokio::spawn({
        let client = wedged.client.clone();
        async move { client.probe().await }
    });

    // When
    let accepted = tokio::time::timeout(LIFECYCLE_DEADLINE, wedged.client.request_stop()).await?;

    // Then
    assert_eq!(accepted?.operation(), "stopping");
    let exit = tokio::time::timeout(LIFECYCLE_DEADLINE, wedged.live.exit()).await??;
    assert_eq!(exit, crate::ipc::ServerExit::ShutdownRequested);
    stalled.abort();
    Ok(())
}
