//! A08: `coordination.terminating` concurrency and withdrawal shortening.

use std::time::Duration;

use interprocess::local_socket::tokio::prelude::*;
use ma2a_core::service::{FrameKind, ProcessReason, ProcessState, ServiceError, Termination};
use ma2a_services::{
    FrameReader, FrameWriter,
    supervisor::{
        EndCause, SupervisorRequest, SupervisorStatus,
        proof::{prove_supervisor, read_challenge},
        status,
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

    // When: creator loses authorization and withdrawal convergence runs.
    harness.withdraw_creator().await?;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kept_unauthorized_reason_survives_second_failing_terminate() -> TestResult {
    // Given: a running Process with two concurrent terminate calls: Call 1 (Unauthorized) gets
    // Ended(Terminated) while the Store fails, then Call 2 (Requested) loses its supervisor reply.
    let harness = Harness::new("term-keep").await?;
    let (manager, shutdown) = harness.manager();
    let id = recorded(&harness, &manager, (52, ProcessState::Running)).await?;
    let record = manager.record(id).await?.ok_or("missing record")?;
    let secret = record.secret.ok_or("missing secret")?;
    let credentials = ma2a_services::supervisor::Credentials::new(id, secret);
    let address = manager.address(id);
    if let Some(parent) = std::path::Path::new(address.as_str()).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = address.bind()?;
    let first_connected = std::sync::Arc::new(Notify::new());
    let second_connected = std::sync::Arc::new(Notify::new());
    let release_first = std::sync::Arc::new(Notify::new());
    let release_second = std::sync::Arc::new(Notify::new());
    let ended_status = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 5);
    let server = tokio::spawn({
        let first_connected = std::sync::Arc::clone(&first_connected);
        let second_connected = std::sync::Arc::clone(&second_connected);
        let release_first = std::sync::Arc::clone(&release_first);
        let release_second = std::sync::Arc::clone(&release_second);
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
            let _req_one = reader_one.expect().await?;
            first_connected.notify_one();

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
            second_connected.notify_one();

            release_first.notified().await;
            writer_one
                .frame(FrameKind::Outcome, &ended_status.to_bytes())
                .await?;
            writer_one.shutdown().await?;

            release_second.notified().await;
            drop((reader_two, writer_two));
            TestResult::Ok(())
        }
    });

    let first_manager = manager.clone();
    let first = tokio::spawn(async move {
        first_manager
            .terminate((None, id), (5_000, ProcessReason::Unauthorized))
            .await
    });
    first_connected.notified().await;

    let second_manager = manager.clone();
    let second = tokio::spawn(async move {
        second_manager
            .terminate((None, id), (5_000, ProcessReason::Requested))
            .await
    });
    second_connected.notified().await;

    let config = ma2a_store::StoreConfig::new(&harness.state.0);
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TRIGGER test_process_end_failure BEFORE UPDATE ON processes
         BEGIN SELECT RAISE(ABORT, 'injected process end failure'); END;",
    )?;

    // When: Call 1 observes Ended(Terminated), fails to commit, and drops its guard before Call 2 fails.
    release_first.notify_one();
    let first_res = tokio::time::timeout(Duration::from_secs(5), first).await??;
    assert_eq!(first_res.err(), Some(ServiceError::Unavailable));

    release_second.notify_one();
    let second_res = tokio::time::timeout(Duration::from_secs(5), second).await??;
    assert_eq!(second_res.err(), Some(ServiceError::Unavailable));

    sql.execute_batch("DROP TRIGGER test_process_end_failure;")?;
    server.await??;

    // Then: after the Store recovers, the follower commits the observed exit with Unauthorized.
    let committed = manager.apply(id, ended_status).await?;
    assert_eq!(committed.value().state, ProcessState::Terminated);
    assert_eq!(committed.value().reason, Some(ProcessReason::Unauthorized));
    assert!(!manager.coordination().terminating.contains_key(&id));
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_unauthorized_terminate_rpc_preserves_reason() -> TestResult {
    // Given: a running Process whose mock supervisor accepts Terminate and drops the connection
    // before sending an Outcome frame.
    let harness = Harness::new("term-rpc-drop").await?;
    let (manager, shutdown) = harness.manager();
    let id = recorded(&harness, &manager, (53, ProcessState::Running)).await?;
    let record = manager.record(id).await?.ok_or("missing record")?;
    let secret = record.secret.ok_or("missing secret")?;
    let credentials = ma2a_services::supervisor::Credentials::new(id, secret);
    let address = manager.address(id);
    if let Some(parent) = std::path::Path::new(address.as_str()).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = address.bind()?;
    let server = tokio::spawn(async move {
        let mut stream = listener.accept().await?;
        let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
        let (recv, send) = stream.split();
        let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
        prove_supervisor(challenge, (&mut reader, &mut writer), &credentials).await?;
        let req = reader.expect().await?;
        assert_eq!(
            SupervisorRequest::from_bytes(req.payload())?,
            SupervisorRequest::Terminate(5_000)
        );
        drop((reader, writer));
        TestResult::Ok(())
    });

    // When: withdrawal terminate receives Unavailable after Terminate was sent, and the follower
    // later applies the supervisor's Ended(Terminated) report.
    let terminated = manager
        .terminate((None, id), (5_000, ProcessReason::Unauthorized))
        .await;
    assert_eq!(terminated.err(), Some(ServiceError::Unavailable));
    server.await??;

    let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 5);
    let committed = manager.apply(id, ended).await?;

    // Then: the Process commits ProcessReason::Unauthorized and clears coordination state.
    assert_eq!(committed.value().state, ProcessState::Terminated);
    assert_eq!(committed.value().reason, Some(ProcessReason::Unauthorized));
    assert!(!manager.coordination().terminating.contains_key(&id));
    shutdown.cancel();
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_conclude_error_preserves_unauthorized_reason() -> TestResult {
    // Given: a running Process whose supervisor is unreachable and whose status path fails with a
    // transient read error (a directory where the status file belongs).
    let harness = Harness::new("term-conclude-err").await?;
    let (manager, shutdown) = harness.manager();
    let id = recorded(&harness, &manager, (54, ProcessState::Running)).await?;
    let record = manager.record(id).await?.ok_or("missing record")?;
    let process_dir = manager.directory(id);
    let status_path = process_dir.join("status");
    std::fs::create_dir_all(&status_path)?;

    // When: withdrawal terminate falls back to conclude and fails transiently, then the status
    // file becomes readable with Ended(Terminated) and the follower retries conclude.
    let terminated = manager
        .terminate((None, id), (5_000, ProcessReason::Unauthorized))
        .await;
    assert_eq!(terminated.err(), Some(ServiceError::Unavailable));

    std::fs::remove_dir(&status_path)?;
    let ended = SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 5);
    status::write(&process_dir, ended)?;
    let committed = manager.conclude(&record).await?;

    // Then: the Process commits ProcessReason::Unauthorized and clears coordination state.
    assert_eq!(committed.value().state, ProcessState::Terminated);
    assert_eq!(committed.value().reason, Some(ProcessReason::Unauthorized));
    assert!(!manager.coordination().terminating.contains_key(&id));
    shutdown.cancel();
    Ok(())
}
