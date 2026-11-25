//! Target-side Proxy v1: one authorized TCP destination relayed byte-exactly.
//!
//! After `accept`, data channel 0 carries the byte stream in both directions and `end`
//! maps to TCP `shutdown(Write)`. The destination was matched against the exposure rule as
//! requested, so resolution here can never widen what the operator approved.

mod relay;

use std::{io::ErrorKind, net::SocketAddr, time::Duration};

use ma2a_core::service::{EMPTY_ACCEPT, ProxyRequest, ProxyResult, ServiceError};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{FrameReader, FrameWriter, exec::reason_error, reply, report::OperationReport};

/// Budget for name resolution plus every connection attempt.
const CONNECT_DEADLINE: Duration = Duration::from_secs(10);

/// Serves one Proxy stream until both directions end or a stop condition fires.
pub async fn serve_proxy<R, W>(
    request: ProxyRequest,
    shutdown: &CancellationToken,
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let connected = tokio::select! {
        biased;
        () = shutdown.cancelled() => return reply::reject(writer, ServiceError::Unavailable).await,
        connected = tokio::time::timeout(CONNECT_DEADLINE, connect(&request)) => connected,
    };
    let stream = match connected {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return reply::reject(writer, error).await,
        Err(_) => return reply::reject(writer, ServiceError::TimedOut).await,
    };
    let started = Instant::now();
    if let Err(error) = reply::accept(writer, &EMPTY_ACCEPT, shutdown).await {
        return reply::lost(error, (0, 0));
    }
    let idle = Duration::from_millis(request.idle_timeout_ms());
    let relayed = relay::run(stream, (reader, &mut *writer), (idle, shutdown)).await;
    let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let bytes = (relayed.to_target, relayed.from_target);
    if let Some(error) = relayed.stream_error {
        return reply::lost(error, bytes);
    }
    let outcome = relayed
        .result
        .map(|reason| ProxyResult::new(reason, bytes, duration_ms));
    let failure = match &outcome {
        Ok(result) => reason_error(result.reason),
        Err(error) => Some(*error),
    };
    let encoded = ProxyResult::encode_outcome(&outcome);
    reply::outcome(writer, (encoded, failure), bytes).await
}

/// Resolves the requested name and connects to the first reachable address.
async fn connect(request: &ProxyRequest) -> Result<TcpStream, ServiceError> {
    let addresses: Vec<SocketAddr> =
        tokio::net::lookup_host((request.host().as_str(), request.port()))
            .await
            .map_err(|_| ServiceError::DnsFailed)?
            .collect();
    let mut failure = ServiceError::DnsFailed;
    for address in addresses {
        match TcpStream::connect(address).await {
            Ok(stream) => {
                let _nodelay = stream.set_nodelay(true);
                return Ok(stream);
            }
            Err(error) => {
                failure = match error.kind() {
                    ErrorKind::ConnectionRefused => ServiceError::ConnectionRefused,
                    ErrorKind::TimedOut => ServiceError::TimedOut,
                    _ => ServiceError::Unreachable,
                };
            }
        }
    }
    Err(failure)
}
