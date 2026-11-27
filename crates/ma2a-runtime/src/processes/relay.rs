//! The relay of one attachment between the caller and the supervisor.

mod upstream;

use ma2a_core::service::{FrameKind, ServiceError, decode_skipped};
use ma2a_services::{FrameReader, FrameWriter, supervisor::SupervisorStatus};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::attach::{CALLER_STALL, Caller, lost};
use upstream::{Upstream, relay_upstream};

/// Longest wait for the supervisor's report once the caller detached.
const DETACH_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// What a relay reports: the supervisor's last status, whether it took the caller's detach,
/// the cursor reached, and bytes in/out.
///
/// A supervisor that took the detach may stop sending retained output and report the status the
/// child has at that moment, final or not, so only a report without one completes.
pub(super) type Relayed = ((SupervisorStatus, bool), u64, (u64, u64));

/// Relays one attachment until the supervisor's `outcome`.
pub(super) async fn relay<SR, SW, R, W>(
    (from_supervisor, to_supervisor): (FrameReader<SR>, FrameWriter<SW>),
    (reader, writer): Caller<'_, R, W>,
    ((stdin, mut cursor), stop): ((bool, u64), &CancellationToken),
) -> Result<Relayed, ServiceError>
where
    SR: AsyncRead + Unpin + Send,
    SW: AsyncWrite + Unpin + Send,
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let mut bytes_in = 0_u64;
    let mut bytes_out = 0_u64;
    let stop_downstream = CancellationToken::new();
    let reported = {
        let downstream = relay_downstream(
            (from_supervisor, writer),
            (&mut cursor, &mut bytes_out),
            &stop_downstream,
        );
        let upstream = relay_upstream((reader, to_supervisor), stdin, &mut bytes_in);
        let grace = tokio::time::sleep(std::time::Duration::MAX);
        tokio::pin!(downstream, grace);
        let mut upstream = Some(Box::pin(upstream));
        let (mut detached, mut told) = (false, false);
        if stop.is_cancelled() {
            return Err(ServiceError::Unavailable);
        }
        loop {
            tokio::select! {
                () = stop.cancelled() => {
                    drop(upstream.take());
                    stop_downstream.cancel();
                    let _finished = downstream.await;
                    break Err(ServiceError::Unavailable);
                }
                reported = &mut downstream => match reported {
                    Ok(Some(status)) => break Ok((status, told)),
                    Ok(None) => break Err(ServiceError::Cancelled),
                    Err(error) => break Err(error),
                },
                ended = async {
                    match upstream.as_mut() {
                        Some(fut) => fut.await,
                        None => std::future::pending().await,
                    }
                }, if !detached => {
                    detached = true;
                    upstream = None;
                    match ended {
                        // Release the supervisor connection immediately, but give any started
                        // caller frame a bounded chance to finish before sending the outcome.
                        Upstream::Stalled => {
                            stop_downstream.cancel();
                            let _finished = downstream.await;
                            break Err(ServiceError::Cancelled);
                        }
                        // Its output, then its outcome or the end of its stream, ends this
                        // within the grace.
                        Upstream::Closed | Upstream::Detached => {
                            told = matches!(ended, Upstream::Detached);
                            grace.as_mut().reset(tokio::time::Instant::now() + DETACH_GRACE);
                        }
                    }
                }
                () = &mut grace, if detached => {
                    stop_downstream.cancel();
                    let _finished = downstream.await;
                    break Err(ServiceError::Cancelled);
                }
            }
        }
    }?;
    Ok((reported, cursor, (bytes_in, bytes_out)))
}

/// Writes one caller-facing frame, finishing an already-started write within `FRAME_GRACE`
/// if `stop` fires after releasing `from_supervisor`. Returns `Ok(false)` when stopped.
async fn write_downstream<SR, F>(
    from_supervisor: &mut Option<FrameReader<SR>>,
    stop: &CancellationToken,
    write: F,
) -> Result<bool, ServiceError>
where
    F: Future<Output = Result<(), ServiceError>>,
{
    if stop.is_cancelled() {
        drop(from_supervisor.take());
        return Ok(false);
    }
    let stall = tokio::time::sleep(CALLER_STALL);
    tokio::pin!(write, stall);
    tokio::select! {
        biased;
        () = stop.cancelled() => {
            drop(from_supervisor.take());
            let _finished = tokio::time::timeout(ma2a_services::reply::FRAME_GRACE, write).await;
            Ok(false)
        }
        written = &mut write => written.map(|()| true),
        () = &mut stall => Err(ServiceError::TimedOut),
    }
}

async fn relay_downstream<SR, W>(
    (from_supervisor, writer): (FrameReader<SR>, &mut FrameWriter<W>),
    (cursor, bytes_out): (&mut u64, &mut u64),
    stop: &CancellationToken,
) -> Result<Option<SupervisorStatus>, ServiceError>
where
    SR: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut from_supervisor = Some(from_supervisor);
    loop {
        let Some(reader) = from_supervisor.as_mut() else {
            return Ok(None);
        };
        let frame = tokio::select! {
            biased;
            () = stop.cancelled() => {
                drop(from_supervisor.take());
                return Ok(None);
            }
            frame = reader.expect() => frame.map_err(lost)?,
        };
        match frame.kind() {
            FrameKind::Data => {
                let (channel, bytes) = frame.data().map_err(lost)?;
                if !write_downstream(&mut from_supervisor, stop, writer.data(channel, bytes))
                    .await?
                {
                    return Ok(None);
                }
                let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                *cursor += length;
                *bytes_out += length;
            }
            FrameKind::Control => {
                let next_cursor =
                    decode_skipped(frame.payload()).map_err(|_| ServiceError::Unavailable)?;
                if !write_downstream(
                    &mut from_supervisor,
                    stop,
                    writer.frame(FrameKind::Control, frame.payload()),
                )
                .await?
                {
                    return Ok(None);
                }
                *cursor = next_cursor;
            }
            FrameKind::Outcome => {
                return SupervisorStatus::from_bytes(frame.payload())
                    .map(Some)
                    .map_err(|_| ServiceError::Unavailable);
            }
            _ => return Err(ServiceError::Unavailable),
        }
    }
}
