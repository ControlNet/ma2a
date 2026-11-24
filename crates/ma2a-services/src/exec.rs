//! Target-side one-shot Exec v1 with streamed output and guaranteed child termination.

mod run;

use std::time::Duration;

use ma2a_core::service::{
    Channel, EMPTY_ACCEPT, EndReason, ExecRequest, FrameKind, ProcessResult, ServiceError,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt as _},
    process::ChildStdin,
    sync::oneshot,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    FrameReader, FrameWriter, process, reply,
    report::{OperationEnd, OperationReport, len64},
};

/// Serves one Exec request until the child and every process in its group are gone.
///
/// `shutdown` is the Runtime's own stop signal; nothing started here outlives it.
pub async fn serve_exec<R, W>(
    request: ExecRequest,
    shutdown: &CancellationToken,
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if shutdown.is_cancelled() {
        return reply::reject(writer, ServiceError::Unavailable).await;
    }
    let started = Instant::now();
    let Some(program) = request.spec().program() else {
        return reply::reject(writer, ServiceError::InvalidInput).await;
    };
    let mut command = match process::command(program, request.spec()) {
        Ok(command) => command,
        Err(error) => return reply::reject(writer, error).await,
    };
    let stdin = if request.stdin() {
        std::process::Stdio::piped()
    } else {
        std::process::Stdio::null()
    };
    let spawned = command
        .stdin(stdin)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let Ok(child) = spawned else {
        return reply::reject(writer, ServiceError::SpawnFailed).await;
    };
    let mut child = process::Spawned::new(child);
    if let Err(error) = reply::accept(writer, &EMPTY_ACCEPT, shutdown).await {
        child.kill_all();
        let _reaped = child.child.wait().await;
        return reply::lost(error, (0, 0));
    }
    let limits = run::Limits {
        deadline: started + Duration::from_millis(request.timeout_ms()),
        shutdown,
    };
    let (cancel, cancelled) = oneshot::channel();
    let (done, finished) = oneshot::channel();
    let stdin = child.child.stdin.take();
    let (input_bytes, run) = tokio::join!(
        inbound(reader, stdin, (cancel, finished)),
        run::outbound((&mut child, writer), (limits, cancelled), done),
    );
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let bytes = (input_bytes, run.output_bytes + run.error_bytes);
    if let Some(error) = run.stream_error {
        return reply::lost(error, bytes);
    }
    let result = ProcessResult::new(run.termination, run.reason, duration_ms)
        .with_bytes(run.output_bytes, run.error_bytes);
    let encoded = ProcessResult::encode_outcome(&Ok(result));
    let mut report = reply::outcome(writer, (encoded, None), bytes).await;
    report.end = reason_end(report.end, run.reason);
    report
}

/// Classifies a delivered outcome for audit by why the process ended.
pub(crate) const fn reason_end(end: OperationEnd, reason: EndReason) -> OperationEnd {
    match (end, reason) {
        (OperationEnd::Completed, EndReason::TimedOut) => {
            OperationEnd::Failed(ServiceError::TimedOut)
        }
        (OperationEnd::Completed, EndReason::Cancelled) => {
            OperationEnd::Failed(ServiceError::Cancelled)
        }
        (OperationEnd::Completed, EndReason::Shutdown) => {
            OperationEnd::Failed(ServiceError::Unavailable)
        }
        (end, _) => end,
    }
}

/// Forwards caller input to the child until the run finishes, returning the bytes forwarded.
///
/// A protocol violation, `cancel`, or stream loss is reported once through `cancel`.
async fn inbound<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
    mut stdin: Option<ChildStdin>,
    (cancel, mut finished): (oneshot::Sender<ServiceError>, oneshot::Receiver<()>),
) -> u64 {
    let mut input_bytes = 0_u64;
    // Set once the child stops taking input; the caller may not know yet and keeps sending.
    let mut closed_by_child = false;
    let failure = loop {
        let frame = tokio::select! {
            _ = &mut finished => return input_bytes,
            frame = reader.next() => frame,
        };
        let frame = match frame {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                drop(stdin.take());
                let _finished = finished.await;
                return input_bytes;
            }
            Err(error) => break error,
        };
        match frame.kind() {
            FrameKind::Data => {
                let Ok((Channel::Input, bytes)) = frame.data() else {
                    break ServiceError::InvalidInput;
                };
                let Some(pipe) = stdin.as_mut() else {
                    if closed_by_child {
                        continue;
                    }
                    break ServiceError::InvalidInput;
                };
                input_bytes += len64(bytes.len());
                let written = tokio::select! {
                    _ = &mut finished => return input_bytes,
                    written = pipe.write_all(bytes) => written,
                };
                if written.is_err() {
                    stdin = None;
                    closed_by_child = true;
                }
            }
            FrameKind::End if frame.end() == Ok(Channel::Input) => {
                stdin = None;
                closed_by_child = false;
            }
            FrameKind::Cancel => break ServiceError::Cancelled,
            _ => break ServiceError::InvalidInput,
        }
    };
    drop(stdin);
    let _signalled = cancel.send(failure);
    let _finished = finished.await;
    input_bytes
}
