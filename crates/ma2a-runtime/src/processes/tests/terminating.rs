//! A08: `coordination.terminating` concurrency and withdrawal shortening.

use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use ma2a_core::service::{FrameKind, ProcessReason, ProcessState, ServiceError, Termination};
use ma2a_services::{
    FrameReader, FrameWriter,
    supervisor::{
        EndCause, SupervisorRequest, SupervisorStatus,
        proof::{prove_supervisor, read_challenge},
    },
};
use tokio::sync::Notify;

use super::{Harness, TestResult, reaches, recovery::recorded, started};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_failing_terminate_preserves_unauthorized_reason() -> TestResult {
    // Given: a running Process whose mock supervisor holds the first Terminate open and drops
    // the second Terminate before answering.
    let harness = Harness::new("term-race").await?;
    let (manager, shutdown) = harness.manager();
    let id = recorded(&harness, &manager, (50, ProcessState::Running)).await?;
    let record = manager.record(id).await?.ok_or("missing record")?;
    let secret = record.secret.ok_or("missing secret")?;
    let credentials = ma2a_services::supervisor::Credentials::new(id, secret);
    let address = manager.address(id);
    if let Some(parent) = std::path::Path::new(address.as_str()).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = address.bind()?;
    let first_connected = std::sync::Arc::new(Notify::new());
    let release_first = std::sync::Arc::new(Notify::new());
    let server = tokio::spawn({
        let first_connected = std::sync::Arc::clone(&first_connected);
        let release_first = std::sync::Arc::clone(&release_first);
        async move {
            let mut stream_one = listener.accept().await?;
            let challenge_one = read_challenge(&mut FrameReader::new(&mut stream_one)).await?;
            let (recv_one, send_one) = stream_one.split();
            let (mut reader_one, mut writer_one) =
                (FrameReader::new(recv_one), FrameWriter::new(send_one));
            prove_supervisor(
                challenge_one,
                (&mut reader_one, &mut writer_one),
                &credentials,
            )
            .await?;
            let req_one = reader_one.expect().await?;
            assert_eq!(
                SupervisorRequest::from_bytes(req_one.payload())?,
                SupervisorRequest::Terminate(5_000)
            );
            first_connected.notify_one();

            // Second connection arrives and fails transiently before answering.
            let mut stream_two = listener.accept().await?;
            let challenge_two = read_challenge(&mut FrameReader::new(&mut stream_two)).await?;
            let (recv_two, send_two) = stream_two.split();
            let (mut reader_two, mut writer_two) =
                (FrameReader::new(recv_two), FrameWriter::new(send_two));
            prove_supervisor(
                challenge_two,
                (&mut reader_two, &mut writer_two),
                &credentials,
            )
            .await?;
            let _req_two = reader_two.expect().await?;
            drop((reader_two, writer_two));

            release_first.notified().await;
            let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 5);
            writer_one
                .frame(FrameKind::Outcome, &ended.to_bytes())
                .await?;
            writer_one.shutdown().await?;
            TestResult::Ok(())
        }
    });

    // When: Unauthorized terminate is in flight while a second Requested terminate fails.
    let first_manager = manager.clone();
    let first = tokio::spawn(async move {
        first_manager
            .terminate((None, id), (5_000, ProcessReason::Unauthorized))
            .await
    });
    first_connected.notified().await;
    let second = manager
        .terminate((None, id), (5_000, ProcessReason::Requested))
        .await;
    assert_eq!(second.err(), Some(ServiceError::Unavailable));
    release_first.notify_one();

    let ended = tokio::time::timeout(Duration::from_secs(5), first).await???;
    server.await??;
    shutdown.cancel();

    // Then: the Process still records ProcessReason::Unauthorized.
    assert_eq!(ended.value().state, ProcessState::Terminated);
    assert_eq!(ended.value().reason, Some(ProcessReason::Unauthorized));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn withdrawn_authorization_shortens_long_requested_terminate() -> TestResult {
    // Given: a SIGTERM-ignoring Process with an active 60 s Requested terminate, whose creator
    // holds no authorization grant.
    let harness = Harness::new("term-withdraw").await?;
    let (manager, shutdown) = harness.manager();
    let record = started(&harness, &manager, (51, "trap '' TERM; sleep 60")).await?;
    reaches(&manager, record.id, ProcessState::Running).await?;

    let req_manager = manager.clone();
    let target = (Some(harness.creator), record.id);
    let requested = tokio::spawn(async move {
        req_manager
            .terminate(target, (60_000, ProcessReason::Requested))
            .await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !manager.coordination().terminating.contains_key(&record.id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "Requested terminate did not enter terminating map"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // When: withdrawal convergence observes the unauthorized creator.
    manager.converge().await;

    // Then: termination completes near WITHDRAWN_GRACE_MS (5 s), not 60 s, as Unauthorized.
    let ended = tokio::time::timeout(Duration::from_secs(12), requested)
        .await
        .map_err(|_| "Requested 60 s terminate was not shortened by withdrawal convergence")???;
    assert_eq!(ended.value().state, ProcessState::Terminated);
    assert_eq!(ended.value().reason, Some(ProcessReason::Unauthorized));
    shutdown.cancel();
    Ok(())
}
