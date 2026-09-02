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

    pub(super) fn persist_enrollment(
        &mut self,
        chain: ma2a_core::SpaceChain,
        owner_address: ma2a_store::ValidatedAddressRecord,
    ) -> Result<(u64, ma2a_core::SpaceChain), RuntimeError> {
        let revision = self
            .repository
            .persist_control_batch(&ma2a_store::ControlBatch::new(
                vec![chain.clone()],
                vec![owner_address],
                Vec::new(),
            ))?
            .map_or_else(|| self.repository.revision(), Ok)?;
        Ok((revision, chain))
    }
}
