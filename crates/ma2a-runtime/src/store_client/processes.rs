//! Process reads and steps on the store thread.

use ma2a_core::service::ServiceError;
use ma2a_store::Repository;
use tokio::sync::oneshot;

use crate::store::{ProcessJob, ServiceStoreCommand, StoreClient, StoreCommand};

impl StoreClient {
    /// Runs one Process job against the repository on the store thread.
    ///
    /// A stopped store answers [`ServiceError::Unavailable`]; nothing ran then.
    pub(crate) async fn processes<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Repository) -> T + Send + 'static,
    ) -> Result<T, ServiceError> {
        let (reply, response) = oneshot::channel();
        let job: ProcessJob = Box::new(move |repository| drop(reply.send(job(repository))));
        self.sender
            .send(StoreCommand::Services(ServiceStoreCommand::Process(job)))
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        response.await.map_err(|_| ServiceError::Unavailable)
    }
}
