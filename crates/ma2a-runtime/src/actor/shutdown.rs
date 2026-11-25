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
        // Dropping a receiver drains it only once: a caller between `reserve` and `send` would
        // land its request after that and wait for a reply forever. Closing first and draining
        // until no slot is reserved drops every late request, so its caller is refused. It
        // also releases the connections waiting for an admission before the endpoint closes.
        self.commands.close();
        while self.commands.recv().await.is_some() {}
        self.enrollment_calls.close();
        while self.enrollment_calls.recv().await.is_some() {}
        self.control_calls.close();
        while self.control_calls.recv().await.is_some() {}
        self.echo_calls.close();
        while self.echo_calls.recv().await.is_some() {}
        let _receiver_count = self
            .events
            .send(RuntimeEvent::shutting_down(self.state.revision));
        self.control_rounds.shutdown().await;
        self.control_tasks.shutdown().await;
        self.echo_tasks.shutdown().await;
        // An exchange cut short here drops its reply, so the caller learns the
        // departure did not complete instead of waiting out the exchange bound.
        self.departure_tasks.shutdown().await;
        let relay_stopped = match self.private_relay_server.take() {
            Some(server) => server.shutdown().await.map_err(|_| {
                crate::error::RuntimeError::new(crate::error::RuntimeErrorKind::Shutdown)
            }),
            None => Ok(()),
        };
        // A failure closing one resource must not skip joining the others.
        let endpoint_closed = self.endpoint.shutdown().await;
        // Nobody drains the observations once the loop has stopped; closing the
        // channel releases an observer that is waiting for room to send.
        self.relay_observations.close();
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
