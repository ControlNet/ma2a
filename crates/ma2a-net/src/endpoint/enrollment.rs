use super::{NetError, RuntimeEndpoint};
use iroh::EndpointAddr;

impl RuntimeEndpoint {
    /// Exchanges one bounded enrollment request over the reserved ALPN.
    ///
    /// # Errors
    ///
    /// Returns [`NetError`] when connection, stream, framing, or response validation fails.
    pub async fn exchange_enrollment(
        &self,
        owner: EndpointAddr,
        request: &[u8],
    ) -> Result<(u8, Vec<Vec<u8>>), NetError> {
        crate::enrollment::exchange(self.router.endpoint(), owner, request).await
    }

    /// Exchanges an exact, owner-idempotent invitation redemption.
    ///
    /// Retries one lost response using identical request bytes, within the same
    /// total deadline. This is not used for member departure or other mutations.
    ///
    /// # Errors
    /// Returns an error for invalid framing or exhausted transport recovery.
    /// Explicit owner denials are returned unchanged and never retried.
    pub async fn exchange_redemption(
        &self,
        owner: EndpointAddr,
        request: &[u8],
    ) -> Result<(u8, Vec<Vec<u8>>), NetError> {
        crate::enrollment::exchange_replayable(self.router.endpoint(), owner, request).await
    }
}
