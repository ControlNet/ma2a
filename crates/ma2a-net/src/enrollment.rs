use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::{EndpointId, MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES, MAX_ENROLLMENT_PAGES};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::{NetError, inbound::BUSY_CLOSE_CODE};
pub(crate) use client::{exchange, exchange_replayable};
use limit::EnrollmentLimiter;

mod client;
mod limit;
#[cfg(test)]
mod limit_test;
#[cfg(test)]
#[path = "enrollment/tests.rs"]
mod tests;

const MAX_ENROLLMENT_REQUEST_BYTES: usize = 4_096;
const ENROLLMENT_IO_TIMEOUT: Duration = Duration::from_secs(10);
const ENROLLMENT_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Enrollment connections one peer may have served at once. Each Runtime has at most 32
/// redemptions and 32 departures pending, but rarely more than a few with one owner; a refused
/// connection reads to the caller as an unreachable owner, which it may retry.
const ENROLLMENT_PER_PEER_LIMIT: usize = 8;
/// Enrollment connections served at once in total: as many as the actor's call queue holds.
const ENROLLMENT_GLOBAL_LIMIT: usize = 32;

/// One authenticated inbound enrollment request for the Runtime actor.
#[derive(Debug)]
pub struct EnrollmentCall {
    remote_endpoint_id: EndpointId,
    request: Vec<u8>,
    reply: oneshot::Sender<(u8, Vec<Vec<u8>>)>,
}

impl EnrollmentCall {
    /// Returns the TLS-authenticated remote Endpoint identity.
    pub const fn remote_endpoint_id(&self) -> EndpointId {
        self.remote_endpoint_id
    }
    /// Returns the bounded request bytes.
    pub fn request(&self) -> &[u8] {
        &self.request
    }
    /// Completes the request with a stable status byte and bounded payload.
    pub fn respond(self, status: u8, pages: Vec<Vec<u8>>) {
        let _unsent = self.reply.send((status, pages));
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EnrollmentHandler {
    calls: mpsc::Sender<EnrollmentCall>,
    limiter: EnrollmentLimiter,
}

impl EnrollmentHandler {
    pub(crate) fn new(calls: mpsc::Sender<EnrollmentCall>) -> Self {
        Self {
            calls,
            limiter: EnrollmentLimiter::default(),
        }
    }
}

impl ProtocolHandler for EnrollmentHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // The ALPN is open to any peer, so a connection over the bound is closed before its
        // stream is awaited or read.
        let remote_endpoint_id = connection.remote_id().into();
        let Some(permit) = self.limiter.try_acquire(remote_endpoint_id) else {
            connection.close(BUSY_CLOSE_CODE.into(), b"");
            return Ok(());
        };
        let result = timeout(ENROLLMENT_EXCHANGE_TIMEOUT, async {
            let Ok(stream) = timeout(ENROLLMENT_IO_TIMEOUT, connection.accept_bi()).await else {
                connection.close(1_u8.into(), b"");
                return Ok(());
            };
            let (mut send, mut receive) = stream?;
            let request = timeout(
                ENROLLMENT_IO_TIMEOUT,
                receive.read_to_end(MAX_ENROLLMENT_REQUEST_BYTES),
            )
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)?;
            let (reply, response) = oneshot::channel();
            if timeout(
                ENROLLMENT_IO_TIMEOUT,
                self.calls.send(EnrollmentCall {
                    remote_endpoint_id,
                    request,
                    reply,
                }),
            )
            .await
            .map_or(true, |result| result.is_err())
            {
                // The Runtime never saw the request, so the candidate may send it again.
                connection.close(BUSY_CLOSE_CODE.into(), b"");
                return Ok(());
            }
            let (status, pages) = timeout(ENROLLMENT_IO_TIMEOUT, response)
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_else(|| {
                    eprintln!("enrollment failed at owner_actor_response_deadline");
                    (255, Vec::new())
                });
            let Ok(page_count) = validate_response(status, &pages) else {
                connection.close(2_u8.into(), b"");
                return Ok(());
            };
            let count = page_count.to_be_bytes();
            timeout(
                ENROLLMENT_IO_TIMEOUT,
                send.write_all(&[status, count[0], count[1]]),
            )
            .await
            .map_err(std::io::Error::other)?
            .map_err(std::io::Error::other)?;
            for page in pages {
                let length = u32::try_from(page.len()).map_err(std::io::Error::other)?;
                timeout(ENROLLMENT_IO_TIMEOUT, send.write_all(&length.to_be_bytes()))
                    .await
                    .map_err(std::io::Error::other)?
                    .map_err(std::io::Error::other)?;
                timeout(ENROLLMENT_IO_TIMEOUT, send.write_all(&page))
                    .await
                    .map_err(std::io::Error::other)?
                    .map_err(std::io::Error::other)?;
            }
            send.finish().map_err(std::io::Error::other)?;
            // Answered: a peer that keeps its connection open holds no slot.
            drop(permit);
            let _closed = timeout(ENROLLMENT_IO_TIMEOUT, connection.closed()).await;
            Ok(())
        })
        .await;
        let Ok(result) = result else {
            connection.close(1_u8.into(), b"");
            return Ok(());
        };
        if result.is_err() {
            eprintln!("enrollment failed at owner_request_or_bootstrap_transfer");
        }
        result
    }
}

fn validate_response(status: u8, pages: &[Vec<u8>]) -> Result<u16, NetError> {
    let page_count = u16::try_from(pages.len()).map_err(|_| NetError::enrollment())?;
    validate_response_count(status, page_count)?;
    if pages
        .iter()
        .any(|page| page.is_empty() || page.len() > MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES)
    {
        return Err(NetError::enrollment());
    }
    Ok(page_count)
}

fn validate_response_count(status: u8, page_count: u16) -> Result<usize, NetError> {
    let page_count = usize::from(page_count);
    if (status == 0 && !(1..=MAX_ENROLLMENT_PAGES).contains(&page_count))
        || (status != 0 && page_count != 0)
    {
        return Err(NetError::enrollment());
    }
    Ok(page_count)
}
