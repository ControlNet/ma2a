use ma2a_core::{EchoError, EndpointId};
use tokio::sync::oneshot;

type EchoCallChannel = (
    EchoCall,
    oneshot::Receiver<Result<(), EchoError>>,
    oneshot::Sender<Vec<u8>>,
    oneshot::Receiver<Result<EchoServiceResponse, EchoError>>,
);

/// Successful crate-private service output returned to the transport.
#[derive(Debug)]
pub struct EchoServiceResponse {
    body: Vec<u8>,
    duration_ms: u16,
}

impl EchoServiceResponse {
    /// Creates one bounded encoded response.
    ///
    /// # Errors
    /// Returns [`EchoError::InvalidInput`] when body or duration bounds are exceeded.
    pub fn new(body: Vec<u8>, duration_ms: u16) -> Result<Self, EchoError> {
        if body.len() > ma2a_core::MAX_WIRE_LEN || duration_ms > ma2a_core::MAX_ECHO_DURATION_MS {
            return Err(EchoError::InvalidInput);
        }
        Ok(Self { body, duration_ms })
    }

    pub(crate) fn into_parts(self) -> (Vec<u8>, u16) {
        (self.body, self.duration_ms)
    }
}

/// One TLS-authenticated inbound call whose body remains unread until admission.
#[derive(Debug)]
pub struct EchoCall {
    remote_endpoint_id: EndpointId,
    admission: oneshot::Sender<Result<(), EchoError>>,
    request: oneshot::Receiver<Vec<u8>>,
    response: oneshot::Sender<Result<EchoServiceResponse, EchoError>>,
}

/// Body and response channels available only after authorization succeeds.
#[derive(Debug)]
pub struct EchoAuthorizedCall {
    request: oneshot::Receiver<Vec<u8>>,
    response: oneshot::Sender<Result<EchoServiceResponse, EchoError>>,
}

impl EchoCall {
    pub(crate) fn channel(remote_endpoint_id: EndpointId) -> EchoCallChannel {
        let (admission, admitted) = oneshot::channel();
        let (request, received) = oneshot::channel();
        let (response, replied) = oneshot::channel();
        (
            Self {
                remote_endpoint_id,
                admission,
                request: received,
                response,
            },
            admitted,
            request,
            replied,
        )
    }

    /// Returns the identity authenticated by Iroh TLS.
    pub const fn remote_endpoint_id(&self) -> EndpointId {
        self.remote_endpoint_id
    }

    /// Admits the peer before exposing body bytes to Runtime code.
    pub fn authorize(self) -> Option<EchoAuthorizedCall> {
        self.admission.send(Ok(())).ok()?;
        Some(EchoAuthorizedCall {
            request: self.request,
            response: self.response,
        })
    }

    /// Rejects the peer with a typed error without exposing or reading body bytes.
    pub fn reject(self, error: EchoError) {
        let _unsent = self.admission.send(Err(error));
    }
}

impl EchoAuthorizedCall {
    /// Waits for the bounded body read by the transport after admission.
    ///
    /// # Errors
    /// Returns [`EchoError::Cancelled`] when the transport drops the request body channel.
    pub async fn request(self) -> Result<(Vec<u8>, EchoResponder), EchoError> {
        let request = self.request.await.map_err(|_| EchoError::Cancelled)?;
        Ok((request, EchoResponder(self.response)))
    }
}

/// Single-use response capability paired with one authorized request.
#[derive(Debug)]
pub struct EchoResponder(oneshot::Sender<Result<EchoServiceResponse, EchoError>>);

impl EchoResponder {
    /// Completes the authorized call exactly once.
    pub fn respond(self, response: Result<EchoServiceResponse, EchoError>) {
        let _unsent = self.0.send(response);
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::EchoError;

    use super::EchoCall;
    use crate::EndpointSecret;

    #[tokio::test]
    async fn unavailable_admission_is_preserved_before_body_read() {
        // Given
        let peer = EndpointSecret::generate().endpoint_id();
        let (call, admission, _request, _response) = EchoCall::channel(peer);

        // When
        call.reject(EchoError::Unavailable);

        // Then
        assert!(matches!(admission.await, Ok(Err(EchoError::Unavailable))));
    }

    #[tokio::test]
    async fn dropping_authorized_runtime_work_cancels_the_transport_response() {
        // Given
        let peer = EndpointSecret::generate().endpoint_id();
        let (call, admission, _request, response) = EchoCall::channel(peer);
        let authorized = call.authorize().expect("admission receiver remains");
        assert!(matches!(admission.await, Ok(Ok(()))));
        let runtime_work = tokio::spawn(authorized.request());
        tokio::task::yield_now().await;

        // When
        runtime_work.abort();
        let _cancelled = runtime_work.await;

        // Then
        assert!(response.await.is_err());
    }
}
