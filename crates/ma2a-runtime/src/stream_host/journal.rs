//! Upload staging journal backed by the durable store.

use ma2a_core::{RequestId, service::ServiceError};
use ma2a_services::StagingJournal;

use crate::store::StoreClient;

/// Journals one upload's staging path in `SQLite` before the file exists.
#[derive(Debug)]
pub(crate) struct StoreJournal {
    pub(crate) store: StoreClient,
    pub(crate) request_id: RequestId,
}

impl StagingJournal for StoreJournal {
    async fn record(&self, path: &str) -> Result<(), ServiceError> {
        let created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(0);
        self.store
            .record_staging((path.to_owned(), self.request_id), created_at_ms)
            .await
    }

    async fn forget(&self, path: &str) {
        self.store.forget_staging(path.to_owned()).await;
    }
}
