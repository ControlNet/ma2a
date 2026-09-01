use super::Actor;
use crate::{error::RuntimeError, state::RuntimeEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShutdownAck {
    pub(crate) revision: u64,
    pub(crate) endpoint_closed: bool,
}

impl Actor {
    pub(super) async fn finish(mut self, clean: bool) -> Result<ShutdownAck, RuntimeError> {
        self.state.ready = false;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::shutting_down(self.state.revision));
        self.control_rounds.shutdown().await;
        self.control_tasks.shutdown().await;
        self.echo_tasks.shutdown().await;
        if let Some(server) = self.private_relay_server.take() {
            server.shutdown().await.map_err(|_| {
                crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Shutdown)
            })?;
        }
        let endpoint_closed = self.endpoint.shutdown().await?;
        self.relay_observer.await?;
        let observation = self.store.observe(&self.state).await;
        let clean_shutdown = if clean {
            Some(self.store.clean_shutdown(self.state.boot_id).await)
        } else {
            None
        };
        let stop = self.store.stop().await;
        let observation_revision = observation?;
        self.state.revision = match clean_shutdown {
            Some(result) => result?,
            None => observation_revision,
        };
        stop?;
        Ok(ShutdownAck {
            revision: self.state.revision,
            endpoint_closed,
        })
    }
}
