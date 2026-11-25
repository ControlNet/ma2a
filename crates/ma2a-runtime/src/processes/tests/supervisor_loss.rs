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

use super::{Harness, MIB, TestResult, request_id};
use crate::processes::serve;

/// Takes one connection, proves itself, reads the `open`, sends `accept` when given, and drops.
async fn stand_in(
    listener: &Listener,
    credentials: &Credentials,
    accept: Option<&[u8]>,
) -> TestResult {
    use interprocess::local_socket::traits::tokio::Stream as _;
    let (receive, send) = listener.accept().await?.split();
    let (mut reader, mut writer) = (FrameReader::new(receive), FrameWriter::new(send));
    let challenge = read_challenge(&mut reader).await?;
    prove_supervisor(challenge, (&mut reader, &mut writer), credentials).await?;
    assert_eq!(reader.expect().await?.kind(), FrameKind::Open);
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
