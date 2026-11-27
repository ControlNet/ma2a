//! How the relay of an attachment ends, against a stand-in supervisor.

use ma2a_core::service::{
    Channel, EndReason, FrameKind, LogFrom, ProcessReply, ProcessReplyOutcome, ProcessRequest,
    ProcessState,
};
use ma2a_services::supervisor::protocol::encode_reading;
use tokio_util::sync::CancellationToken;

use super::{
    Harness, TestResult, pipe_with, request_id,
    supervisor_loss::{cuttable, exited, proven, taken_over},
};
use crate::processes::serve;

/// Attaches to a stand-in supervisor that admits a window ending at 1000, sends 3 bytes,
/// waits for the detach, and only then reports that the child exited; the caller detaches
/// after those 3 bytes. With `cut`, the Store stops answering before the exit is reported.
async fn detached_as_it_ends(cut: bool, byte: u8) -> TestResult<ProcessReplyOutcome> {
    let harness = Harness::new("relay-detach").await?;
    let (store, cutter) = cuttable(&harness.store);
    let (manager, id, credentials, listener) = taken_over(&harness, store, byte).await?;
    let supervisor = async {
        let (mut reader, mut writer) = proven(&listener, &credentials).await?;
        writer
            .frame(FrameKind::Accept, &encode_reading((0, 0, 1000), false))
            .await?;
        writer.data(Channel::Output, b"abc").await?;
        writer.flush().await?;
        assert_eq!(reader.expect().await?.kind(), FrameKind::Cancel);
        if cut {
            cutter.cancel();
        }
        writer.frame(FrameKind::Outcome, &exited(0)).await?;
        writer.flush().await?;
        TestResult::Ok(())
    };
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let stop = CancellationToken::new();
    let request = ProcessRequest::Attach(id, LogFrom::Start, false);
    let scope = ((harness.creator, request_id(byte + 1)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let caller = async {
        assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
        assert_eq!(
            reader.expect().await?.data()?,
            (Channel::Output, &b"abc"[..])
        );
        writer.frame(FrameKind::Cancel, &[]).await?;
        writer.flush().await?;
        let outcome = reader.expect().await?;
        assert_eq!(outcome.kind(), FrameKind::Outcome);
        TestResult::Ok(ProcessReply::decode_outcome(outcome.payload())?)
    };
    let joined = async { tokio::join!(host, caller, supervisor) };
    let (_report, answered, stood_in) =
        tokio::time::timeout(std::time::Duration::from_secs(30), joined).await?;
    stood_in?;
    answered
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_that_detached_as_the_process_ended_is_told_it_detached() -> TestResult {
    // Given: a Process that ends while output the caller never read is still retained.
    // When: the caller detaches.
    let outcome = detached_as_it_ends(false, 80).await?;

    // Then: it detached where its bytes ended, and the summary shows the exit.
    let Ok(ProcessReply::Detached(reason, summary, cursor)) = outcome else {
        return Err(format!("expected a detach, got {outcome:?}").into());
    };
    assert_eq!((reason, cursor), (EndReason::Cancelled, 3));
    assert_eq!(summary.state, ProcessState::Exited);

    // Given: the same, while the Store no longer answers.
    // When
    let outcome = detached_as_it_ends(true, 82).await?;

    // Then: a detach needs no record of the exit.
    assert!(
        matches!(
            outcome,
            Ok(ProcessReply::Detached(EndReason::Cancelled, _, 3))
        ),
        "{outcome:?}"
    );
    Ok(())
}

/// Shuts down the receiving side of a stand-in's connection, as a supervisor that stopped
/// reading its input would, while its sending side stays open.
fn stop_reading(
    reader: ma2a_services::FrameReader<interprocess::local_socket::tokio::RecvHalf>,
) -> TestResult<interprocess::local_socket::tokio::RecvHalf> {
    use std::os::fd::AsFd as _;
    let receive = reader.into_inner();
    let interprocess::local_socket::tokio::RecvHalf::UdSocket(socket) = &receive;
    let socket = std::os::unix::net::UnixStream::from(socket.as_fd().try_clone_to_owned()?);
    socket.shutdown(std::net::Shutdown::Read)?;
    Ok(receive)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_caller_can_detach_from_a_silent_supervisor_that_stopped_taking_input() -> TestResult {
    // Given: an attachment with stdin whose supervisor stops reading and then sends nothing.
    let harness = Harness::new("relay-half-closed").await?;
    let (manager, id, credentials, listener) =
        taken_over(&harness, harness.store.clone(), 84).await?;
    let (stopped, gone) = tokio::sync::oneshot::channel();
    let (answered, done) = tokio::sync::oneshot::channel::<()>();
    let supervisor = async {
        let (reader, mut writer) = proven(&listener, &credentials).await?;
        writer
            .frame(FrameKind::Accept, &encode_reading((0, 0, 0), false))
            .await?;
        writer.flush().await?;
        let _receive = stop_reading(reader)?;
        stopped.send(()).map_err(|()| "the caller is gone")?;
        // Silent, with its sending side open, until the caller has its answer.
        let _answered = done.await;
        drop(writer);
        TestResult::Ok(())
    };
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let stop = CancellationToken::new();
    let request = ProcessRequest::Attach(id, LogFrom::Now, true);
    let scope = ((harness.creator, request_id(85)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let caller = async {
        assert_eq!(reader.expect().await?.kind(), FrameKind::Accept);
        gone.await?;
        // When: input finds the supervisor no longer reading, and the caller then detaches.
        writer.data(Channel::Input, b"x").await?;
        writer.flush().await?;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        writer.frame(FrameKind::Cancel, &[]).await?;
        writer.flush().await?;
        let outcome = reader.expect().await;
        let _sent = answered.send(());
        let outcome = outcome?;
        assert_eq!(outcome.kind(), FrameKind::Outcome);
        TestResult::Ok(ProcessReply::decode_outcome(outcome.payload())?)
    };
    let joined = async { tokio::join!(host, caller, supervisor) };
    let (_report, answered, stood_in) =
        tokio::time::timeout(std::time::Duration::from_secs(15), joined).await?;
    stood_in?;

    // Then: the detach ends the attachment within the grace, freeing its reader slot.
    assert_eq!(answered?, Err(ma2a_core::service::ServiceError::Cancelled));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_continuous_output_flood_never_starves_caller_detach() -> TestResult {
    // Given: a stand-in supervisor continuously flooding 64-byte output frames while the
    // caller's sink drains without backpressure.
    let harness = Harness::new("relay-fair-detach").await?;
    let (manager, id, credentials, listener) =
        taken_over(&harness, harness.store.clone(), 88).await?;
    let stop_flood = CancellationToken::new();
    let flood_token = stop_flood.clone();
    let supervisor = async {
        use tokio::io::AsyncWriteExt as _;
        let (mut reader, mut writer) = proven(&listener, &credentials).await?;
        writer
            .frame(FrameKind::Accept, &encode_reading((0, 0, 0), false))
            .await?;
        writer.flush().await?;
        let mut batch = ma2a_services::FrameWriter::new(Vec::new());
        for _ in 0..256 {
            batch.data(Channel::Output, &[b'o'; 64]).await?;
        }
        let batch = batch.into_inner();
        let flood = tokio::spawn(async move {
            while !flood_token.is_cancelled() {
                if writer.get_mut().write_all(&batch).await.is_err() {
                    break;
                }
            }
            writer
        });
        let frame =
            tokio::time::timeout(std::time::Duration::from_secs(1), reader.expect()).await??;
        assert_eq!(frame.kind(), FrameKind::Cancel);
        stop_flood.cancel();
        let mut writer = flood.await?;
        writer.frame(FrameKind::Outcome, &exited(0)).await?;
        writer.flush().await?;
        TestResult::Ok(())
    };
    let (caller_stream, host_stream) = tokio::io::duplex(1 << 20);
    let (_caller_read, caller_write) = tokio::io::split(caller_stream);
    let (host_read, _host_write) = tokio::io::split(host_stream);
    let mut writer = ma2a_services::FrameWriter::new(caller_write);
    let mut host_reader = ma2a_services::FrameReader::new(host_read);
    let mut host_writer = ma2a_services::FrameWriter::new(tokio::io::sink());
    let stop = CancellationToken::new();
    let request = ProcessRequest::Attach(id, LogFrom::Now, false);
    let scope = ((harness.creator, request_id(89)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let caller = async {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        writer.frame(FrameKind::Cancel, &[]).await?;
        writer.flush().await?;
        TestResult::Ok(())
    };
    let (report, caller, supervisor) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(host, caller, supervisor)
        })
        .await?;
    caller?;
    supervisor.map_err(|error| format!("supervisor failed ({report:?}): {error}"))?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_caller_cancel_or_eof_while_stdin_write_is_blocked_ends_at_once() -> TestResult {
    use ma2a_core::service::ServiceError;
    use ma2a_services::{FrameReader, FrameWriter};
    use tokio::io::AsyncWriteExt as _;

    #[derive(Clone, Copy, Debug)]
    enum Mode {
        Cancel,
        Eof,
        EndThenCancel,
    }

    for mode in [Mode::Cancel, Mode::Eof, Mode::EndThenCancel] {
        let (caller, host) = tokio::io::duplex(4_096);
        let (_caller_read, caller_write) = tokio::io::split(caller);
        let (host_read, _host_write) = tokio::io::split(host);
        let mut writer = FrameWriter::new(caller_write);
        let mut reader = FrameReader::new(host_read);

        // Tiny supervisor pipe so the very first 128-byte Data frame blocks `to_supervisor.data`.
        let (supervisor, pipe_sup) = tokio::io::duplex(64);
        let (sup_read, _sup_write) = tokio::io::split(supervisor);
        let (pipe_read, pipe_write) = tokio::io::split(pipe_sup);
        let mut from_sup = FrameReader::new(pipe_read);
        let mut to_sup = FrameWriter::new(pipe_write);

        let relay = crate::processes::stdin::relay_stdin((&mut from_sup, &mut to_sup), &mut reader);
        let caller_task = async {
            writer.data(Channel::Input, &[b'x'; 128]).await?;
            writer.flush().await?;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            match mode {
                Mode::Cancel => {
                    writer.frame(FrameKind::Cancel, &[]).await?;
                    writer.flush().await?;
                }
                Mode::Eof => writer.get_mut().shutdown().await?,
                Mode::EndThenCancel => {
                    writer.end(Channel::Input).await?;
                    writer.frame(FrameKind::Cancel, &[]).await?;
                    writer.flush().await?;
                }
            }
            TestResult::Ok(())
        };

        let started = tokio::time::Instant::now();
        let (relayed, sent) = tokio::join!(relay, caller_task);
        sent?;
        drop(sup_read);
        assert_eq!(relayed, Err(ServiceError::Cancelled), "mode={mode:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "mode={mode:?} waited {:?}",
            started.elapsed()
        );
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_caller_cancel_or_eof_while_attach_stdin_write_is_blocked_ends_at_once() -> TestResult {
    use ma2a_core::service::ServiceError;
    use ma2a_services::{FrameReader, FrameWriter};
    use tokio::io::AsyncWriteExt as _;

    #[derive(Clone, Copy, Debug)]
    enum Mode {
        Cancel,
        Eof,
        EndThenCancel,
    }

    for mode in [Mode::Cancel, Mode::Eof, Mode::EndThenCancel] {
        let (caller, host) = tokio::io::duplex(4_096);
        let (_caller_read, caller_write) = tokio::io::split(caller);
        let (host_read, host_write) = tokio::io::split(host);
        let mut writer = FrameWriter::new(caller_write);
        let mut reader = FrameReader::new(host_read);
        let mut host_writer = FrameWriter::new(host_write);

        // Tiny supervisor pipe so the very first 128-byte Data frame blocks `to_supervisor.data`.
        let (supervisor, pipe_sup) = tokio::io::duplex(64);
        let (sup_read, _sup_write) = tokio::io::split(supervisor);
        let (pipe_read, pipe_write) = tokio::io::split(pipe_sup);
        let from_sup = FrameReader::new(pipe_read);
        let to_sup = FrameWriter::new(pipe_write);
        let stop = CancellationToken::new();

        let relay = crate::processes::relay::relay(
            (from_sup, to_sup),
            (&mut reader, &mut host_writer),
            ((true, 0), &stop),
        );
        let caller_task = async {
            writer.data(Channel::Input, &[b'x'; 128]).await?;
            writer.flush().await?;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            match mode {
                Mode::Cancel => {
                    writer.frame(FrameKind::Cancel, &[]).await?;
                    writer.flush().await?;
                }
                Mode::Eof => writer.get_mut().shutdown().await?,
                Mode::EndThenCancel => {
                    writer.end(Channel::Input).await?;
                    writer.frame(FrameKind::Cancel, &[]).await?;
                    writer.flush().await?;
                }
            }
            TestResult::Ok(())
        };

        let started = tokio::time::Instant::now();
        let (relayed, sent) = tokio::join!(relay, caller_task);
        sent?;
        drop(sup_read);
        assert_eq!(relayed, Err(ServiceError::Cancelled), "mode={mode:?}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "mode={mode:?} waited {:?}",
            started.elapsed()
        );
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn cancel_while_both_writes_block_finishes_output_frame_and_releases_supervisor() -> TestResult
{
    use ma2a_core::service::{ProcessReply, ServiceError};
    use ma2a_services::{FrameReader, FrameWriter};
    use tokio::io::AsyncReadExt as _;

    // Two tiny 64-byte buffers force both a 128-byte output write and a 128-byte stdin write
    // to stall mid-frame.
    let (caller, host) = tokio::io::duplex(64);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (host_read, host_write) = tokio::io::split(host);
    let mut caller_reader = FrameReader::new(caller_read);
    let mut caller_writer = FrameWriter::new(caller_write);
    let mut host_reader = FrameReader::new(host_read);
    let mut host_writer = FrameWriter::new(host_write);

    let (supervisor, pipe_sup) = tokio::io::duplex(64);
    let (mut sup_read, sup_write) = tokio::io::split(supervisor);
    let (pipe_read, pipe_write) = tokio::io::split(pipe_sup);
    let mut sup_writer = FrameWriter::new(sup_write);
    let from_sup = FrameReader::new(pipe_read);
    let to_sup = FrameWriter::new(pipe_write);
    let stop = CancellationToken::new();

    let host = async move {
        let relayed = crate::processes::relay::relay(
            (from_sup, to_sup),
            (&mut host_reader, &mut host_writer),
            ((true, 0), &stop),
        )
        .await;
        if let Err(error) = relayed {
            let _report = crate::processes::host::finish(&mut host_writer, Err(error)).await;
        }
        host_writer.is_torn()
    };

    let (sup_sent_tx, sup_sent_output) = tokio::sync::oneshot::channel();
    let (cancel_tx, cancel_sent) = tokio::sync::oneshot::channel();
    let (sup_released_tx, sup_released) = tokio::sync::oneshot::channel();
    let supervisor_task = async {
        sup_writer.data(Channel::Output, &[b'o'; 128]).await?;
        sup_writer.flush().await?;
        let _sent = sup_sent_tx.send(());
        // Do not read `sup_read` until after `Cancel`, forcing `to_supervisor.data` to block
        // mid-frame. Once `Cancel` is sent, both supervisor halves must be dropped promptly even
        // before the caller drains output.
        cancel_sent.await?;
        let mut drained = [0_u8; 64];
        let mut total = 0_usize;
        loop {
            let n = sup_read.read(&mut drained).await?;
            if n == 0 {
                break;
            }
            total += n;
        }
        assert!(total < 128, "expected partial stdin write, got {total}");
        let _released = sup_released_tx.send(());
        TestResult::Ok(())
    };

    let caller_task = async {
        sup_sent_output.await?;
        caller_writer.data(Channel::Input, &[b'x'; 128]).await?;
        caller_writer.flush().await?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        caller_writer.frame(FrameKind::Cancel, &[]).await?;
        caller_writer.flush().await?;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let _sent = cancel_tx.send(());

        // Supervisor connection closes before output is drained.
        sup_released.await?;
        let data = caller_reader.expect().await?;
        assert_eq!(data.data()?, (Channel::Output, &[b'o'; 128][..]));
        let outcome = caller_reader.expect().await?;
        assert_eq!(outcome.kind(), FrameKind::Outcome);
        assert_eq!(
            ProcessReply::decode_outcome(outcome.payload())?,
            Err(ServiceError::Cancelled)
        );
        TestResult::Ok(())
    };

    let (torn, sup_res, caller_res) =
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            tokio::join!(host, supervisor_task, caller_task)
        })
        .await?;
    sup_res?;
    caller_res?;
    assert!(!torn, "downstream writer left torn");
    Ok(())
}
