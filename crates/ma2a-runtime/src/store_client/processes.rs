//! Process reads and steps on the store thread.

use ma2a_core::{EndpointId, service::ServiceError};
use ma2a_store::Repository;
use tokio::sync::oneshot;

use crate::store::{GoDispatchLease, ProcessJob, ServiceStoreCommand, StoreClient, StoreCommand};

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

    /// Checks `Process` authorization and holds the Store thread until `lease` completes
    /// or expires, preventing a withdrawal from committing between the check and `go` dispatch.
    pub(crate) async fn authorize_process_go(
        &self,
        (local, peer): (EndpointId, EndpointId),
        lease: GoDispatchLease,
    ) -> Result<(), ServiceError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::Services(
                ServiceStoreCommand::AuthorizeProcessGo {
                    endpoints: (local, peer),
                    lease,
                    reply,
                },
            ))
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        response.await.map_err(|_| ServiceError::Unavailable)?
    }

    #[cfg(all(test, unix))]
    pub(crate) async fn set_go_dispatch_hold(&self, hold: std::time::Duration) {
        let (reply, response) = oneshot::channel();
        if self
            .sender
            .send(StoreCommand::Services(
                ServiceStoreCommand::SetGoDispatchHold { hold, reply },
            ))
            .await
            .is_ok()
        {
            let _ack = response.await;
        }
    }
}
