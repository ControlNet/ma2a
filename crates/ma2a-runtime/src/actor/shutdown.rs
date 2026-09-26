use super::Actor;
use crate::{error::RuntimeError, state::RuntimeEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShutdownAck {
    pub(crate) revision: u64,
    pub(crate) endpoint_closed: bool,
}

impl Actor {
    pub(crate) async fn finish(mut self, clean: bool) -> Result<ShutdownAck, RuntimeError> {
        self.state.ready = false;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::shutting_down(self.state.revision));
        self.control_rounds.shutdown().await;
        self.control_tasks.shutdown().await;
        self.echo_tasks.shutdown().await;
        let relay_stopped = match self.private_relay_server.take() {
            Some(server) => server.shutdown().await.map_err(|_| {
                crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Shutdown)
            }),
            None => Ok(()),
        };
        // A failure closing one resource must not skip joining the others.
        let endpoint_closed = self.endpoint.shutdown().await;
        let observer_stopped = self.relay_observer.await;
        let observation = self.store.observe(&self.state).await;
        let clean_shutdown = if clean
            && relay_stopped.is_ok()
            && endpoint_closed.is_ok()
            && observer_stopped.is_ok()
        {
            Some(self.store.clean_shutdown(self.state.boot_id).await)
        } else {
            None
        };
        let stop = self.store.stop().await;
        relay_stopped?;
        let endpoint_closed = endpoint_closed?;
        observer_stopped?;
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
