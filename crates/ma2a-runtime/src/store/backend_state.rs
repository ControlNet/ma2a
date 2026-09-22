use ma2a_store::RuntimeMetadataUpdate;

use super::StoreBackend;
use crate::error::RuntimeError;

impl StoreBackend {
    pub(super) fn set_endpoint_bind_port(&mut self, port: u16) -> Result<u64, RuntimeError> {
        self.repository
            .set_endpoint_bind_port(port)
            .map_err(Into::into)
    }

    pub(super) fn record_metadata(
        &mut self,
        update: RuntimeMetadataUpdate,
    ) -> Result<u64, RuntimeError> {
        self.repository
            .record_runtime_metadata(&update)
            .map_err(Into::into)
    }

    /// Persists an enrollment result and reports the membership the Store now holds.
    ///
    /// Membership is read back rather than assumed: a batch that the Store
    /// legitimately refuses, such as a replayed older chain, must not leave the
    /// Runtime believing it joined.
    pub(super) fn persist_enrollment(
        &mut self,
        request: super::EnrollmentPersistence,
    ) -> Result<super::PersistedEnrollment, RuntimeError> {
        let revision = self
            .repository
            .persist_control_batch(&ma2a_store::ControlBatch::new(
                vec![request.chain.clone()],
                vec![*request.owner_address],
                Vec::new(),
            ))?
            .map_or_else(|| self.repository.revision(), Ok)?;
        Ok(super::PersistedEnrollment {
            revision,
            chain: request.chain,
            memberships: self.repository.memberships_for(request.local_endpoint_id)?,
        })
    }
}
