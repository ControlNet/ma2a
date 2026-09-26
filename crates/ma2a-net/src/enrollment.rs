use iroh::{
    EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::{EndpointId, MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES, MAX_ENROLLMENT_PAGES};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Duration, timeout};

use crate::{ENROLLMENT_ALPN, NetError};

#[cfg(test)]
#[path = "enrollment/tests.rs"]
mod tests;

const MAX_ENROLLMENT_REQUEST_BYTES: usize = 4_096;
const ENROLLMENT_IO_TIMEOUT: Duration = Duration::from_secs(10);
const ENROLLMENT_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);

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
}

impl EnrollmentHandler {
    pub(crate) const fn new(calls: mpsc::Sender<EnrollmentCall>) -> Self {
        Self { calls }
    }
}

impl ProtocolHandler for EnrollmentHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let result = timeout(ENROLLMENT_EXCHANGE_TIMEOUT, async {
            let remote_endpoint_id = connection.remote_id().into();
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
                connection.close(1_u8.into(), b"");
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

pub(crate) async fn exchange(
    endpoint: &iroh::Endpoint,
    owner: EndpointAddr,
    request: &[u8],
) -> Result<(u8, Vec<Vec<u8>>), NetError> {
    timeout(
        ENROLLMENT_EXCHANGE_TIMEOUT,
        exchange_bounded(endpoint, owner, request),
    )
    .await
    .map_err(|_| NetError::enrollment_at("exchange_deadline"))?
}

async fn exchange_bounded(
    endpoint: &iroh::Endpoint,
    owner: EndpointAddr,
    request: &[u8],
) -> Result<(u8, Vec<Vec<u8>>), NetError> {
    if request.len() > MAX_ENROLLMENT_REQUEST_BYTES {
        return Err(NetError::enrollment_at("exchange_deadline"));
    }
    let connection = timeout(
        ENROLLMENT_IO_TIMEOUT,
        endpoint.connect(owner, ENROLLMENT_ALPN),
    )
    .await
    .map_err(|_| NetError::enrollment_at("owner_dial_and_alpn"))?
    .map_err(|_| NetError::enrollment_at("owner_dial_and_alpn"))?;
    let (mut send, mut receive) = timeout(ENROLLMENT_IO_TIMEOUT, connection.open_bi())
        .await
        .map_err(|_| NetError::enrollment_at("request_stream"))?
        .map_err(|_| NetError::enrollment_at("request_stream"))?;
    timeout(ENROLLMENT_IO_TIMEOUT, send.write_all(request))
        .await
        .map_err(|_| NetError::enrollment_at("request_transfer"))?
        .map_err(|_| NetError::enrollment_at("request_transfer"))?;
    send.finish()
        .map_err(|_| NetError::enrollment_at("request_transfer"))?;
    let mut header = [0_u8; 3];
    timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut header))
        .await
        .map_err(|_| NetError::enrollment_at("bootstrap_header"))?
        .map_err(|_| NetError::enrollment_at("bootstrap_header"))?;
    let page_count =
        validate_response_count(header[0], u16::from_be_bytes([header[1], header[2]]))?;
    let mut pages = Vec::with_capacity(page_count);
    for _page_index in 0..page_count {
        let mut length = [0_u8; 4];
        timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut length))
            .await
            .map_err(|_| NetError::enrollment_at("bootstrap_transfer"))?
            .map_err(|_| NetError::enrollment_at("bootstrap_transfer"))?;
        let length = usize::try_from(u32::from_be_bytes(length))
            .map_err(|_| NetError::enrollment_at("bootstrap_transfer"))?;
        if length == 0 || length > MAX_ENROLLMENT_BOOTSTRAP_FRAME_BYTES {
            return Err(NetError::enrollment_at("bootstrap_transfer"));
        }
        let mut page = vec![0_u8; length];
        timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut page))
            .await
            .map_err(|_| NetError::enrollment_at("bootstrap_transfer"))?
            .map_err(|_| NetError::enrollment_at("bootstrap_transfer"))?;
        pages.push(page);
    }
    let mut trailing = [0_u8; 1];
    match timeout(ENROLLMENT_IO_TIMEOUT, receive.read_exact(&mut trailing)).await {
        Ok(Err(iroh::endpoint::ReadExactError::FinishedEarly(0))) => {}
        Ok(Ok(()) | Err(_)) | Err(_) => return Err(NetError::enrollment_at("bootstrap_eof")),
    }
    connection.close(0_u8.into(), b"");
    Ok((header[0], pages))
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
