//! Target-side interactive Shell v1 on a PTY (Unix) or `ConPTY` (Windows).
//!
//! Sessions are ephemeral: they end on exit, lifetime expiry, caller cancel or loss, and
//! Runtime shutdown, and never survive a restart.

mod io;
mod pty;
#[cfg(unix)]
mod reap;

use std::time::Duration;

use ma2a_core::service::{EMPTY_ACCEPT, EndReason, ProcessResult, ShellRequest, Termination};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::oneshot,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{FrameReader, FrameWriter, exec::reason_error, reply, report::OperationReport};

/// Serves one Shell session until its program is gone.
pub async fn serve_shell<R, W>(
    request: ShellRequest,
    shutdown: &CancellationToken,
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    if shutdown.is_cancelled() {
        return reply::reject(writer, ma2a_core::service::ServiceError::Unavailable).await;
    }
    let started = Instant::now();
    let mut session = match pty::spawn(&request) {
        Ok(session) => session,
        Err(error) => return reply::reject(writer, error).await,
    };
    if let Err(error) = reply::accept(writer, &EMPTY_ACCEPT, shutdown).await {
        session.close(true).await;
        return reply::lost(error, (0, 0));
    }
    let (cancel, cancelled) = oneshot::channel();
    let (done, finished) = oneshot::channel();
    let pty::Session {
        master,
        output,
        input,
        exited,
        ..
    } = &mut session;
    let outbound = io::Outbound {
        output,
        exited,
        deadline: started + Duration::from_millis(request.lifetime_ms()),
        shutdown,
        cancelled,
        done,
    };
    let (input_bytes, ended) = tokio::join!(
        io::inbound(reader, (input.clone(), master), (cancel, finished)),
        io::outbound(outbound, writer),
    );
    let leader_running =
        ended.reason != EndReason::Completed || ended.termination == Termination::Unknown;
    session.close(leader_running).await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let bytes = (input_bytes, ended.output_bytes);
    if let Some(error) = ended.stream_error {
        return reply::lost(error, bytes);
    }
    let reason = ended.reason;
    let result = ProcessResult::new(ended.termination, reason, duration_ms)
        .with_bytes(ended.output_bytes, 0);
    let encoded = ProcessResult::encode_outcome(&Ok(result));
    reply::outcome(writer, (encoded, reason_error(reason)), bytes).await
}
