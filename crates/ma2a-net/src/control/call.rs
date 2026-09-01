use ma2a_core::EndpointId;
use tokio::sync::oneshot;

use super::ControlRejection;

type ControlCallChannel = (
    ControlCall,
    oneshot::Receiver<Result<(), ControlRejection>>,
    oneshot::Sender<Vec<u8>>,
    oneshot::Receiver<Result<Vec<u8>, ControlRejection>>,
);

/// One TLS-authenticated inbound control call whose body remains unread until admission.
#[derive(Debug)]
pub struct ControlCall {
    remote_endpoint_id: EndpointId,
    admission: oneshot::Sender<Result<(), ControlRejection>>,
    request: oneshot::Receiver<Vec<u8>>,
    response: oneshot::Sender<Result<Vec<u8>, ControlRejection>>,
}

/// Body and response channels available only after shared-Space authorization succeeds.
#[derive(Debug)]
pub struct ControlAuthorizedCall {
    request: oneshot::Receiver<Vec<u8>>,
    response: oneshot::Sender<Result<Vec<u8>, ControlRejection>>,
}

impl ControlCall {
    pub(crate) fn channel(remote_endpoint_id: EndpointId) -> ControlCallChannel {
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

    /// Returns the Endpoint identity authenticated by Iroh TLS.
    pub const fn remote_endpoint_id(&self) -> EndpointId {
        self.remote_endpoint_id
    }

    /// Admits the peer before exposing or reading request bytes.
    pub fn authorize(self) -> Option<ControlAuthorizedCall> {
        self.admission.send(Ok(())).ok()?;
        Some(ControlAuthorizedCall {
            request: self.request,
            response: self.response,
        })
    }

    /// Rejects the peer without exposing or reading request bytes.
    pub fn reject(self, rejection: ControlRejection) {
        let _unsent = self.admission.send(Err(rejection));
    }
}

impl ControlAuthorizedCall {
    /// Waits for the bounded request body read by the transport after admission.
    ///
    /// # Errors
    /// Returns [`ControlRejection::Unavailable`] when the transport drops the body channel.
    pub async fn request(self) -> Result<(Vec<u8>, ControlResponder), ControlRejection> {
        let request = self
            .request
            .await
            .map_err(|_| ControlRejection::Unavailable)?;
        Ok((request, ControlResponder(self.response)))
    }
}

/// Single-use response capability paired with one authorized control request.
#[derive(Debug)]
pub struct ControlResponder(oneshot::Sender<Result<Vec<u8>, ControlRejection>>);

impl ControlResponder {
    /// Completes the authorized call exactly once.
    pub fn respond(self, response: Result<Vec<u8>, ControlRejection>) {
        let _unsent = self.0.send(response);
    }
}

#[cfg(test)]
mod tests {
    use super::ControlCall;
    use crate::{ControlRejection, EndpointSecret};

    #[tokio::test]
    async fn unavailable_admission_is_preserved_before_body_read() {
        // Given
        let peer = EndpointSecret::generate().endpoint_id();
        let (call, admission, _request, _response) = ControlCall::channel(peer);

        // When
        call.reject(ControlRejection::Unavailable);

        // Then
        assert!(matches!(
            admission.await,
            Ok(Err(ControlRejection::Unavailable))
        ));
    }
}
