use iroh::{
    EndpointAddr,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use ma2a_core::EndpointId;
use tokio::sync::{mpsc, oneshot};

use crate::{ENROLLMENT_ALPN, NetError};

/// Maximum enrollment request or response bytes accepted on one stream.
pub const MAX_ENROLLMENT_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

/// One authenticated inbound enrollment request for the Runtime actor.
#[derive(Debug)]
pub struct EnrollmentCall {
    remote_endpoint_id: EndpointId,
    request: Vec<u8>,
    reply: oneshot::Sender<(u8, Vec<u8>)>,
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
    pub fn respond(self, status: u8, payload: Vec<u8>) {
        let _unsent = self.reply.send((status, payload));
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
        let remote_endpoint_id = connection.remote_id().into();
        let (mut send, mut receive) = connection.accept_bi().await?;
        let request = receive
            .read_to_end(MAX_ENROLLMENT_MESSAGE_BYTES)
            .await
            .map_err(std::io::Error::other)?;
        let (reply, response) = oneshot::channel();
        if self
            .calls
            .send(EnrollmentCall {
                remote_endpoint_id,
                request,
                reply,
            })
            .await
            .is_err()
        {
            connection.close(1_u8.into(), b"");
            return Ok(());
        }
        let (status, payload) = response.await.unwrap_or((255, Vec::new()));
        if payload.len() >= MAX_ENROLLMENT_MESSAGE_BYTES {
            connection.close(2_u8.into(), b"");
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(payload.len() + 1);
        bytes.push(status);
        bytes.extend_from_slice(&payload);
        send.write_all(&bytes)
            .await
            .map_err(std::io::Error::other)?;
        send.finish().map_err(std::io::Error::other)?;
        connection.closed().await;
        Ok(())
    }
}

pub(crate) async fn exchange(
    endpoint: &iroh::Endpoint,
    owner: EndpointAddr,
    request: &[u8],
) -> Result<(u8, Vec<u8>), NetError> {
    if request.len() >= MAX_ENROLLMENT_MESSAGE_BYTES {
        return Err(NetError::enrollment());
    }
    let connection = endpoint
        .connect(owner, ENROLLMENT_ALPN)
        .await
        .map_err(|_| NetError::enrollment())?;
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .map_err(|_| NetError::enrollment())?;
    send.write_all(request)
        .await
        .map_err(|_| NetError::enrollment())?;
    send.finish().map_err(|_| NetError::enrollment())?;
    let response = receive
        .read_to_end(MAX_ENROLLMENT_MESSAGE_BYTES)
        .await
        .map_err(|_| NetError::enrollment())?;
    connection.close(0_u8.into(), b"");
    let (&status, payload) = response.split_first().ok_or_else(NetError::enrollment)?;
    Ok((status, payload.to_vec()))
}
