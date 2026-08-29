use std::collections::BTreeSet;

use ma2a_core::SpaceId;
use ma2a_net::RuntimeEndpoint;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    state::{RuntimeEvent, RuntimeStatus},
    store::StoreClient,
};

pub(crate) const COMMAND_CAPACITY: usize = 32;
pub(crate) const EVENT_CAPACITY: usize = 32;

pub(crate) enum Command {
    Status(oneshot::Sender<RuntimeStatus>),
    ObserveMemberships {
        memberships: Vec<SpaceId>,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    Shutdown(oneshot::Sender<ShutdownAck>),
}

/// Bounded command and event handle for the single-owner Runtime actor.
#[derive(Clone, Debug)]
pub struct RuntimeHandle {
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<RuntimeEvent>,
}

impl RuntimeHandle {
    pub(crate) const fn new(
        commands: mpsc::Sender<Command>,
        events: broadcast::Sender<RuntimeEvent>,
    ) -> Self {
        Self { commands, events }
    }

    /// Returns an authoritative state snapshot through the actor mailbox.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when the Runtime actor has stopped.
    pub async fn status(&self) -> Result<RuntimeStatus, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Status(reply))
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }

    /// Replaces the observed valid membership set without rebuilding the Endpoint.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] when persistence fails or the Runtime actor has stopped.
    pub async fn observe_memberships(
        &self,
        memberships: Vec<SpaceId>,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::ObserveMemberships { memberships, reply })
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?
    }

    /// Subscribes to bounded best-effort Runtime events.
    pub fn subscribe(&self) -> broadcast::Receiver<RuntimeEvent> {
        self.events.subscribe()
    }

    pub(crate) async fn shutdown(&self) -> Result<ShutdownAck, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Shutdown(reply))
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))?;
        response
            .await
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Channel))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShutdownAck {
    pub(crate) revision: u64,
    pub(crate) endpoint_closed: bool,
}

pub(crate) struct Actor {
    state: RuntimeStatus,
    endpoint: RuntimeEndpoint,
    store: StoreClient,
    commands: mpsc::Receiver<Command>,
    events: broadcast::Sender<RuntimeEvent>,
    cancellation: CancellationToken,
}

impl Actor {
    pub(crate) fn new(
        state: RuntimeStatus,
        endpoint: RuntimeEndpoint,
        store: StoreClient,
    ) -> (Self, RuntimeHandle, CancellationToken) {
        let (command_sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let cancellation = CancellationToken::new();
        let handle = RuntimeHandle::new(command_sender, events.clone());
        let actor = Self {
            state,
            endpoint,
            store,
            commands,
            events,
            cancellation: cancellation.child_token(),
        };
        (actor, handle, cancellation)
    }

    pub(crate) async fn run(mut self) -> Result<ShutdownAck, RuntimeError> {
        let _receiver_count = self.events.send(RuntimeEvent::ready(self.state.revision));
        loop {
            tokio::select! {
                biased;
                () = self.cancellation.cancelled() => return self.finish(false).await,
                command = self.commands.recv() => match command {
                    Some(Command::Status(reply)) => {
                        let _unsent = reply.send(self.state.clone());
                    }
                    Some(Command::ObserveMemberships { memberships, reply }) => {
                        let result = self.observe_memberships(memberships).await;
                        let _unsent = reply.send(result);
                    }
                    Some(Command::Shutdown(reply)) => {
                        let result = self.finish(true).await;
                        if let Ok(ack) = result {
                            let _unsent = reply.send(ack);
                            return Ok(ack);
                        }
                        return result;
                    }
                    None => return self.finish(false).await,
                }
            }
        }
    }

    async fn observe_memberships(
        &mut self,
        memberships: Vec<SpaceId>,
    ) -> Result<u64, RuntimeError> {
        let mut candidate = self.state.clone();
        candidate.memberships = memberships.into_iter().collect::<BTreeSet<_>>();
        let revision = self.store.observe(&candidate).await?;
        candidate.revision = revision;
        self.state = candidate;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::memberships_changed(revision));
        Ok(revision)
    }

    async fn finish(mut self, clean: bool) -> Result<ShutdownAck, RuntimeError> {
        self.state.ready = false;
        let _receiver_count = self
            .events
            .send(RuntimeEvent::shutting_down(self.state.revision));
        let endpoint_closed = self.endpoint.shutdown().await?;
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
