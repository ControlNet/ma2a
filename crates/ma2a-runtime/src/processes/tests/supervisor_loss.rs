//! A supervisor connection lost in the middle of an operation is this Runtime's failure.
//!
//! A stand-in takes over the supervisor's socket path, proves itself with the Process's
//! credentials, and drops the connection at a chosen point.

use interprocess::local_socket::{tokio::Listener, traits::tokio::Listener as _};
use ma2a_core::service::{
    FrameKind, LogFrom, ProcessId, ProcessRequest, ProcessSignal, ProcessSpec, ProcessStart,
    ServiceError,
};
use ma2a_services::{
    FrameReader, FrameWriter,
    client::{ClientError, hosted, process::ProcessIo},
    supervisor::{
        Credentials,
        proof::{prove_supervisor, read_challenge},
        protocol::{ACKNOWLEDGED, encode_reading},
    },
};
use ma2a_store::ProcessCreation;
use tokio_util::sync::CancellationToken;

use super::{Harness, MIB, TestResult, pipe_with, request_id};
use crate::processes::serve;

/// One proven supervisor connection, as the stand-in holds it.
pub(super) type Proven = (
    FrameReader<interprocess::local_socket::tokio::RecvHalf>,
    FrameWriter<interprocess::local_socket::tokio::SendHalf>,
);

/// Takes one connection, proves itself, and reads the `open`.
pub(super) async fn proven(listener: &Listener, credentials: &Credentials) -> TestResult<Proven> {
    use interprocess::local_socket::traits::tokio::Stream as _;
    let (receive, send) = listener.accept().await?.split();
    let (mut reader, mut writer) = (FrameReader::new(receive), FrameWriter::new(send));
    let challenge = read_challenge(&mut reader).await?;
    prove_supervisor(challenge, (&mut reader, &mut writer), credentials).await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Open);
    Ok((reader, writer))
}

/// Takes one connection, proves itself, reads the `open`, sends `accept` when given, and drops.
async fn stand_in(
    listener: &Listener,
    credentials: &Credentials,
    accept: Option<&[u8]>,
) -> TestResult {
    let (_reader, mut writer) = proven(listener, credentials).await?;
    if let Some(accept) = accept {
        writer.frame(FrameKind::Accept, accept).await?;
        writer.flush().await?;
    }
    Ok(())
}

/// Runs one stream operation as the creator while the stand-in answers.
async fn run(
    harness: &Harness,
    (manager, listener, credentials): (&super::ProcessManager, &Listener, &Credentials),
    (byte, request, accept): (u8, ProcessRequest, Option<&[u8]>),
) -> TestResult<Result<(), ClientError>> {
    let (caller, host) = tokio::io::duplex(1 << 16);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (host_read, host_write) = tokio::io::split(host);
    let (mut reader, mut writer) = (
        FrameReader::new(caller_read),
        FrameWriter::new(caller_write),
    );
    let (mut host_reader, mut host_writer) =
        (FrameReader::new(host_read), FrameWriter::new(host_write));
    let stop = CancellationToken::new();
    let scope = ((harness.creator, request_id(byte)?), request.clone(), &stop);
    let host = serve(manager, scope, (&mut host_reader, &mut host_writer));
    let caller = async {
        match request {
            ProcessRequest::Stdin(_, close) => {
                hosted::stdin((&mut reader, &mut writer), (&b"x"[..], close))
                    .await
                    .map(|_| ())
            }
            ProcessRequest::Attach(id, from, _) => {
                let local = ProcessIo::<&[u8], Vec<u8>, Vec<u8>> {
                    input: None,
                    output: Vec::new(),
                    error: None,
                };
                let detach = std::future::pending();
                hosted::attach((&mut reader, &mut writer), ((id, from), local), detach)
                    .await
                    .map(|_| ())
            }
            _ => hosted::call(&mut reader).await.map(|_| ()),
        }
    };
    let (_report, answer, stood_in) =
        tokio::join!(host, caller, stand_in(listener, credentials, accept));
    stood_in?;
    Ok(answer)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_supervisor_connection_is_unavailable_not_cancelled() -> TestResult {
    // Given: a running Process with stdin whose socket path a stand-in took over.
    let harness = Harness::new("supervisor-loss").await?;
    let (manager, shutdown) = harness.manager();
    let spec = ProcessSpec::new(
        Some("/bin/cat".to_owned()),
        Vec::new(),
        Some("/".to_owned()),
    )?;
    let start = ProcessStart::new(spec, true, MIB)?;
    let binding = (harness.creator, request_id(50)?);
    let (creation, credentials) = manager.create(binding, &start).await?;
    let ProcessCreation::Created(record) = creation else {
        return Err("expected a new Process".into());
    };
    let id: ProcessId = record.id;
    manager.bring_up(credentials, start).await?;
    let address = manager.address(id);
    std::fs::remove_file(address.as_str())?;
    let listener = address.bind()?;
    let context = (&manager, &listener, &credentials);
    let reading = encode_reading((0, 0, 0), false);

    for (byte, request, accept, refused) in [
        // Lost before the supervisor answered: refused, never the caller's doing.
        (
            51,
            ProcessRequest::Attach(id, LogFrom::Now, false),
            None,
            true,
        ),
        // Lost while attached, while writing stdin, and before a signal was acknowledged.
        (
            52,
            ProcessRequest::Attach(id, LogFrom::Now, false),
            Some(reading.as_slice()),
            false,
        ),
        (
            53,
            ProcessRequest::Stdin(id, false),
            Some(ACKNOWLEDGED),
            false,
        ),
        (
            54,
            ProcessRequest::Signal(id, ProcessSignal::Interrupt),
            None,
            false,
        ),
    ] {
        // When
        let answer = run(&harness, context, (byte, request.clone(), accept)).await?;

        // Then
        let unavailable = if refused {
            matches!(
                answer,
                Err(ClientError::Rejected(ServiceError::Unavailable))
            )
        } else {
            matches!(answer, Err(ClientError::Failed(ServiceError::Unavailable)))
        };
        assert!(unavailable, "{request:?}: {answer:?}");
    }
    std::fs::remove_file(address.as_str())?;
    shutdown.cancel();
    Ok(())
}

/// A Store client whose requests stop reaching the Store once the returned token is cancelled;
/// each one after that fails as an unreachable Store does.
pub(super) fn cuttable(
    store: &crate::store::StoreClient,
) -> (crate::store::StoreClient, CancellationToken) {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(crate::store::STORE_CAPACITY);
    let cut = CancellationToken::new();
    let (real, stop) = (store.sender.clone(), cut.clone());
    tokio::spawn(async move {
        while let Some(command) = receiver.recv().await {
            // Dropped, so its reply never comes.
            if !stop.is_cancelled() && real.send(command).await.is_err() {
                break;
            }
        }
    });
    (crate::store::StoreClient::new(sender), cut)
}

/// A running `/bin/cat` Process with stdin whose socket path a stand-in took over, managed
/// through `store`; returns its manager, id, credentials, and the stand-in's listener.
pub(super) async fn taken_over(
    harness: &Harness,
    store: crate::store::StoreClient,
    byte: u8,
) -> TestResult<(super::ProcessManager, ProcessId, Credentials, Listener)> {
    let manager = super::ProcessManager::new(
        (
            harness.local,
            store,
            std::sync::Arc::new(crate::clock::SystemClock),
        ),
        (&harness.state.0, Some(super::ProcessHosting::in_process())),
        CancellationToken::new(),
    );
    let spec = ProcessSpec::new(
        Some("/bin/cat".to_owned()),
        Vec::new(),
        Some("/".to_owned()),
    )?;
    let start = ProcessStart::new(spec, true, MIB)?;
    let (creation, credentials) = manager
        .create((harness.creator, request_id(byte)?), &start)
        .await?;
    let ProcessCreation::Created(record) = creation else {
        return Err("expected a new Process".into());
    };
    manager.bring_up(credentials, start).await?;
    let address = manager.address(record.id);
    std::fs::remove_file(address.as_str())?;
    let listener = address.bind()?;
    Ok((manager, record.id, credentials, listener))
}

/// The status a supervisor reports once its child exited with `code`.
pub(super) fn exited(code: u32) -> Vec<u8> {
    ma2a_services::supervisor::status::SupervisorStatus::Ended(
        ma2a_core::service::Termination::Exited(code),
        ma2a_services::supervisor::status::EndCause::Exited,
        1,
    )
    .to_bytes()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_exit_that_could_not_be_recorded_is_no_completed_attachment() -> TestResult {
    // Given: an attachment whose supervisor reports the exit once the Store stopped answering.
    let harness = Harness::new("attach-unrecorded").await?;
    let (store, cut) = cuttable(&harness.store);
    let (manager, id, credentials, listener) = taken_over(&harness, store, 60).await?;
    let supervisor = async {
        let (_reader, mut writer) = proven(&listener, &credentials).await?;
        writer
            .frame(FrameKind::Accept, &encode_reading((0, 0, 0), false))
            .await?;
        writer.flush().await?;
        cut.cancel();
        writer.frame(FrameKind::Outcome, &exited(7)).await?;
        writer.flush().await?;
        TestResult::Ok(())
    };

    // When
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1 << 16);
    let stop = CancellationToken::new();
    let request = ProcessRequest::Attach(id, LogFrom::Now, false);
    let scope = ((harness.creator, request_id(61)?), request, &stop);
    let host = serve(&manager, scope, (&mut host_reader, &mut host_writer));
    let local = ProcessIo::<&[u8], Vec<u8>, Vec<u8>> {
        input: None,
        output: Vec::new(),
        error: None,
    };
    let target = (id, LogFrom::Now);
    let caller = hosted::attach(
        (&mut reader, &mut writer),
        (target, local),
        std::future::pending(),
    );
    let (_report, attached, stood_in) = tokio::join!(host, caller, supervisor);
    stood_in?;

    // Then: the exit is not reported over a summary that still says running.
    assert!(
        matches!(
            attached,
            Err(ClientError::Failed(ServiceError::Unavailable))
        ),
        "{attached:?}"
    );
    Ok(())
}

/// Attaches with stdin to a Process whose stand-in supervisor sends 16 KiB of output, then the
/// child's exit when `exits`, and closes its connection while the caller takes nothing; only then
/// does the caller send input and read. Returns the output bytes the caller got and its outcome.
async fn attach_while_closed(
    exits: bool,
    byte: u8,
) -> TestResult<(usize, ma2a_core::service::ProcessReplyOutcome)> {
    use ma2a_core::service::{Channel, ProcessReply};
    let harness = Harness::new("attach-closed").await?;
    let (manager, id, credentials, listener) =
        taken_over(&harness, harness.store.clone(), byte).await?;
    let (closed, gone) = tokio::sync::oneshot::channel();
    let supervisor = async {
        let (reader, mut writer) = proven(&listener, &credentials).await?;
        writer
            .frame(FrameKind::Accept, &encode_reading((0, 0, 0), false))
            .await?;
        for _ in 0..4 {
            writer.data(Channel::Output, &[b'o'; 4_096]).await?;
        }
        if exits {
            writer.frame(FrameKind::Outcome, &exited(0)).await?;
        }
        writer.flush().await?;
        drop((reader, writer));
        closed.send(()).map_err(|()| "the caller is gone")?;
        TestResult::Ok(())
    };
    // The caller's stream holds less than the output, so the relay is still sending it.
    let ((mut reader, mut writer), (mut host_reader, mut host_writer)) = pipe_with(1024);
    let stop = CancellationToken::new();
    let request = ProcessRequest::Attach(id, LogFrom::Now, true);
    let scope = ((harness.creator, request_id(byte + 1)?), request, &stop);
    let manager = &manager;
    // The host's stream closes once it answered, as the Runtime's does.
    let host = async move {
        serve(manager, scope, (&mut host_reader, &mut host_writer)).await;
    };
    let caller = async {
        gone.await?;
        writer.data(Channel::Input, b"x").await?;
        writer.flush().await?;
        let mut received = 0;
        loop {
            let frame = reader.expect().await?;
            match frame.kind() {
                FrameKind::Accept => {}
                FrameKind::Data => received += frame.data()?.1.len(),
                FrameKind::Outcome => {
                    return TestResult::Ok((
                        received,
                        ProcessReply::decode_outcome(frame.payload())?,
                    ));
                }
                other => return Err(format!("unexpected {other:?}").into()),
            }
        }
    };
    let joined = async { tokio::join!(host, caller, supervisor) };
    let (_report, answered, stood_in) =
        tokio::time::timeout(std::time::Duration::from_secs(30), joined).await?;
    stood_in?;
    answered
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_supervisor_that_closed_still_delivers_what_it_sent_to_a_writing_caller() -> TestResult {
    use ma2a_core::service::{EndReason, ProcessReply};
    // Given: a supervisor that sent output and the child's exit, then closed.
    // When: the caller's input finds the connection closed.
    let (received, outcome) = attach_while_closed(true, 72).await?;

    // Then: every byte and the exit still arrive.
    assert_eq!(received, 16_384);
    assert!(
        matches!(
            outcome,
            Ok(ProcessReply::Detached(EndReason::Completed, _, 16_384))
        ),
        "{outcome:?}"
    );

    // Given: a supervisor that closed without reporting the exit.
    let (received, outcome) = attach_while_closed(false, 74).await?;

    // Then: its output arrives, and its loss is this Runtime's failure, not a detach.
    assert_eq!(received, 16_384);
    assert_eq!(outcome, Err(ServiceError::Unavailable));
    Ok(())
}
