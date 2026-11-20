//! Output streaming, stop conditions, and child termination for one Exec run.

use std::{process::ExitStatus, time::Duration};

use ma2a_core::service::{Channel, EndReason, MAX_DATA_CHUNK_LEN, ServiceError, Termination};
use tokio::{
    io::{AsyncRead, AsyncReadExt as _, AsyncWrite},
    sync::oneshot,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    FrameWriter,
    process::{self, Spawned},
    report::len64,
};

/// How long output is still drained after the child exits before its group is killed.
const EXIT_DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Stop conditions other than caller cancellation.
pub(super) struct Limits<'a> {
    pub(super) deadline: Instant,
    pub(super) shutdown: &'a CancellationToken,
}

/// Terminal facts of one run.
#[derive(Debug)]
pub(super) struct Run {
    pub(super) termination: Termination,
    pub(super) reason: EndReason,
    pub(super) output_bytes: u64,
    pub(super) error_bytes: u64,
    pub(super) stream_error: Option<ServiceError>,
}

/// Reads the next output chunk; zero bytes is end of output, and an error is never mistaken
/// for it.
async fn read_some<P: AsyncRead + Unpin>(
    pipe: Option<&mut P>,
    buffer: &mut [u8],
) -> std::io::Result<usize> {
    match pipe {
        Some(pipe) => pipe.read(buffer).await,
        None => std::future::pending().await,
    }
}

/// How the child ended: its exit status when waiting for it succeeded, `Unknown` otherwise.
fn ended(status: Option<&std::io::Result<ExitStatus>>) -> Termination {
    match status {
        Some(Ok(status)) => process::termination(*status),
        Some(Err(_)) | None => Termination::Unknown,
    }
}

async fn stop_reason(
    limits: Limits<'_>,
    cancelled: oneshot::Receiver<ServiceError>,
) -> (EndReason, Option<ServiceError>) {
    let cancelled = async {
        match cancelled.await {
            Ok(ServiceError::Cancelled) => None,
            Ok(error) => Some(error),
            Err(_) => std::future::pending().await,
        }
    };
    tokio::select! {
        () = limits.shutdown.cancelled() => (EndReason::Shutdown, None),
        () = tokio::time::sleep_until(limits.deadline) => (EndReason::TimedOut, None),
        error = cancelled => (EndReason::Cancelled, error),
    }
}

/// Streams output until exit plus a short drain, or until a stop condition; then kills.
pub(super) async fn outbound<W: AsyncWrite + Unpin>(
    (spawned, writer): (&mut Spawned, &mut FrameWriter<W>),
    (limits, cancelled): (Limits<'_>, oneshot::Receiver<ServiceError>),
    done: oneshot::Sender<()>,
) -> Run {
    let (mut stdout, mut stderr) = (spawned.child.stdout.take(), spawned.child.stderr.take());
    let mut buffers = (
        vec![0_u8; MAX_DATA_CHUNK_LEN],
        vec![0_u8; MAX_DATA_CHUNK_LEN],
    );
    let stop = stop_reason(limits, cancelled);
    tokio::pin!(stop);
    let mut run = Run {
        termination: Termination::Unknown,
        reason: EndReason::Completed,
        output_bytes: 0,
        error_bytes: 0,
        stream_error: None,
    };
    let mut exit: Option<std::io::Result<ExitStatus>> = None;
    let mut drain_until: Option<Instant> = None;
    loop {
        if exit.is_some() && stdout.is_none() && stderr.is_none() {
            break;
        }
        let (channel, read) = tokio::select! {
            biased;
            (reason, error) = &mut stop => {
                run.reason = reason;
                run.stream_error = error;
                break;
            }
            status = spawned.child.wait(), if exit.is_none() => {
                exit = Some(status);
                drain_until = Some(Instant::now() + EXIT_DRAIN_GRACE);
                continue;
            }
            () = tokio::time::sleep_until(drain_until.unwrap_or_else(Instant::now)), if drain_until.is_some() => break,
            read = read_some(stdout.as_mut(), &mut buffers.0), if stdout.is_some() => (Channel::Output, read),
            read = read_some(stderr.as_mut(), &mut buffers.1), if stderr.is_some() => (Channel::Error, read),
        };
        // Output that could not be read is lost, so the run cannot report complete output.
        let Ok(read) = read else {
            run.reason = EndReason::Cancelled;
            run.stream_error = Some(ServiceError::Internal);
            break;
        };
        if read == 0 {
            match channel {
                Channel::Output => stdout = None,
                Channel::Error | Channel::Input => stderr = None,
            }
            continue;
        }
        let bytes = match channel {
            Channel::Output => buffers.0.get(..read),
            Channel::Error | Channel::Input => buffers.1.get(..read),
        }
        .unwrap_or_default();
        let written = tokio::select! {
            biased;
            (reason, error) = &mut stop => {
                run.reason = reason;
                run.stream_error = error;
                break;
            }
            written = writer.data(channel, bytes) => written,
        };
        if let Err(error) = written {
            run.reason = EndReason::Cancelled;
            run.stream_error = Some(error);
            break;
        }
        match channel {
            Channel::Output => run.output_bytes += len64(read),
            Channel::Error | Channel::Input => run.error_bytes += len64(read),
        }
    }
    // Process-group IDs are not reused while any member lives, and PIDs are allocated
    // sequentially, so killing the group right after the leader exits cannot reach an
    // unrelated process group in practice.
    spawned.kill_all();
    let status = match exit {
        Some(status) => Some(status),
        None => Some(spawned.child.wait().await),
    };
    run.termination = ended(status.as_ref());
    let _finished = done.send(());
    run
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use ma2a_core::service::Termination;
    use tokio::io::{AsyncRead, ReadBuf};

    use super::{ended, read_some};

    /// A pipe that fails on every read.
    struct Broken;

    impl AsyncRead for Broken {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
    }

    #[tokio::test]
    async fn a_failed_read_is_not_end_of_output() {
        let mut buffer = [0_u8; 8];
        assert!(read_some(Some(&mut Broken), &mut buffer).await.is_err());
        let mut empty: &[u8] = &[];
        assert_eq!(read_some(Some(&mut empty), &mut buffer).await.ok(), Some(0));
    }

    #[test]
    fn a_failed_wait_is_an_unknown_termination() {
        assert_eq!(
            ended(Some(&Err(std::io::ErrorKind::Other.into()))),
            Termination::Unknown
        );
        assert_eq!(ended(None), Termination::Unknown);
    }
}
