//! The relay of one attachment between the caller and the supervisor.

use ma2a_core::service::{Channel as DataChannel, FrameKind, ServiceError, decode_skipped};
use ma2a_services::supervisor::{Channel, SupervisorStatus};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::attach::{CALLER_STALL, Caller, lost};

/// Longest wait for the supervisor's report once the caller detached.
const DETACH_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// What a relay reports: the supervisor's last status, the cursor reached, bytes in/out.
pub(super) type Relayed = (SupervisorStatus, u64, (u64, u64));

/// How the caller's side of a relay ended.
enum Upstream {
    /// The supervisor took the detach.
    Detached,
    /// The supervisor's connection is closed; what it sent before still follows.
    Closed,
    /// The supervisor did not take input or the detach in time.
    Stalled,
}

/// Relays one attachment until the supervisor's `outcome`.
pub(super) async fn relay<R, W>(
    (mut from_supervisor, mut to_supervisor): Channel,
    (reader, writer): Caller<'_, R, W>,
    ((stdin, mut cursor), stop): ((bool, u64), &CancellationToken),
) -> Result<Relayed, ServiceError>
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let mut bytes_in = 0_u64;
    let mut bytes_out = 0_u64;
    let reported = {
        let downstream = async {
            loop {
                let frame = from_supervisor.expect().await.map_err(lost)?;
                match frame.kind() {
                    FrameKind::Data => {
                        let (channel, bytes) = frame.data().map_err(lost)?;
                        tokio::time::timeout(CALLER_STALL, writer.data(channel, bytes))
                            .await
                            .map_err(|_| ServiceError::TimedOut)??;
                        let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        cursor += length;
                        bytes_out += length;
                    }
                    FrameKind::Control => {
                        cursor = decode_skipped(frame.payload())
                            .map_err(|_| ServiceError::Unavailable)?;
                        tokio::time::timeout(
                            CALLER_STALL,
                            writer.frame(FrameKind::Control, frame.payload()),
                        )
                        .await
                        .map_err(|_| ServiceError::TimedOut)??;
                    }
                    FrameKind::Outcome => {
                        return SupervisorStatus::from_bytes(frame.payload())
                            .map_err(|_| ServiceError::Unavailable);
                    }
                    _ => return Err(ServiceError::Unavailable),
                }
            }
        };
        let upstream = async {
            // Returning detaches: the supervisor then reports where the Process is. Input the
            // Process does not take within the stall bound detaches too, so a caller's detach
            // is never stuck behind it. A supervisor that closed its connection, for example
            // once the child ended, takes nothing more but still has its output delivered.
            while let Ok(Some(frame)) = reader.next().await {
                let forwarded = match (frame.kind(), frame.data(), frame.end()) {
                    (FrameKind::Data, Ok((DataChannel::Input, bytes)), _) if stdin => {
                        bytes_in += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        let write = to_supervisor.data(DataChannel::Input, bytes);
                        tokio::time::timeout(CALLER_STALL, write).await
                    }
                    (FrameKind::End, _, Ok(DataChannel::Input)) if stdin => {
                        let write = to_supervisor.end(DataChannel::Input);
                        tokio::time::timeout(CALLER_STALL, write).await
                    }
                    _ => break,
                };
                match forwarded {
                    Ok(Ok(())) => {}
                    Ok(Err(ServiceError::Cancelled)) => return Upstream::Closed,
                    // A write cut short by the stall bound left a torn frame: no detach follows.
                    Err(_) => return Upstream::Stalled,
                    Ok(Err(_)) => break,
                }
            }
            let cancel = to_supervisor.frame(FrameKind::Cancel, &[]);
            match tokio::time::timeout(DETACH_GRACE, cancel).await {
                Ok(Ok(())) => Upstream::Detached,
                Ok(Err(_)) => Upstream::Closed,
                Err(_) => Upstream::Stalled,
            }
        };
        let grace = tokio::time::sleep(std::time::Duration::MAX);
        tokio::pin!(downstream, upstream, grace);
        let mut detached = false;
        loop {
            tokio::select! {
                biased;
                () = stop.cancelled() => break Err(ServiceError::Unavailable),
                reported = &mut downstream => break reported,
                ended = &mut upstream, if !detached => {
                    detached = true;
                    match ended {
                        // A detach the supervisor did not take has used up the grace already.
                        Upstream::Stalled => break Err(ServiceError::Cancelled),
                        // Its output, then its outcome or the end of its stream, ends this.
                        Upstream::Closed => {}
                        Upstream::Detached => {
                            grace.as_mut().reset(tokio::time::Instant::now() + DETACH_GRACE);
                        }
                    }
                }
                () = &mut grace, if detached => break Err(ServiceError::Cancelled),
            }
        }
    }?;
    Ok((reported, cursor, (bytes_in, bytes_out)))
}
