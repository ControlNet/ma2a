use super::{Command, EndpointId, RuntimeHandle, oneshot};

impl RuntimeHandle {
    /// Calls the encrypted typed Echo service without semantic retry.
    ///
    /// # Errors
    /// Returns a typed Echo failure for invalid input, authorization, timeout, or transport errors.
    #[expect(
        clippy::too_many_arguments,
        reason = "the public Echo call requires correlation, target identity, and payload"
    )]
    pub async fn echo(
        &self,
        request_id: ma2a_core::RequestId,
        target: EndpointId,
        payload: &[u8],
    ) -> Result<ma2a_core::EchoResponse<'static>, ma2a_core::EchoError> {
        Ok(self
            .echo_committed(request_id, target, payload)
            .await?
            .into_value())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Echo requires correlation, target and payload"
    )]
    pub(crate) async fn echo_committed(
        &self,
        request_id: ma2a_core::RequestId,
        target: EndpointId,
        payload: &[u8],
    ) -> Result<ma2a_store::Committed<ma2a_core::EchoResponse<'static>>, ma2a_core::EchoError> {
        if payload.len() > ma2a_core::MAX_ECHO_PAYLOAD_LEN {
            return Err(ma2a_core::EchoError::InvalidInput);
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Echo {
                request_id,
                target,
                payload: payload.to_vec(),
                reply,
            })
            .await
            .map_err(|_| ma2a_core::EchoError::Cancelled)?;
        response
            .await
            .map_err(|_| ma2a_core::EchoError::Cancelled)?
    }

    /// Returns the bounded payload-free Echo audit snapshot.
    pub fn echo_audit(&self) -> Vec<crate::EchoAuditRecord> {
        self.echo_audit.snapshot()
    }

    /// Returns body-processing metrics for authorization-ordering verification.
    pub fn echo_metrics(&self) -> ma2a_net::EchoMetricsSnapshot {
        self.echo_metrics.snapshot()
    }
}
