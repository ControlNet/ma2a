use std::{collections::BTreeSet, sync::Arc};

use ma2a_core::SpaceId;
use ma2a_net::{
    ControlCall, EnrollmentCall, IrohRelayObservation, RuntimeEndpoint, SpaceAddressLookup,
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::state::{RuntimeEvent, RuntimeStatus};
use crate::{enrollment::EnrollmentError, error::RuntimeError, store::StoreClient};

mod command;
mod handle;
mod local_control;
pub(crate) use command::Command;
pub use handle::RuntimeHandle;

pub(crate) const COMMAND_CAPACITY: usize = 32;
pub(crate) const EVENT_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ShutdownAck {
    pub(crate) revision: u64,
    pub(crate) endpoint_closed: bool,
}

pub(crate) struct Actor {
    pub(crate) state: RuntimeStatus,
    pub(crate) endpoint: RuntimeEndpoint,
    pub(crate) store: StoreClient,
    commands: mpsc::Receiver<Command>,
    pub(crate) events: broadcast::Sender<RuntimeEvent>,
    pub(crate) clock: Arc<dyn crate::RuntimeClock>,
    enrollment_calls: mpsc::Receiver<EnrollmentCall>,
    pub(crate) control_calls: mpsc::Receiver<ControlCall>,
    pub(crate) lookup: SpaceAddressLookup,
    relay_observations: mpsc::Receiver<IrohRelayObservation>,
    relay_observer: tokio::task::JoinHandle<()>,
    pub(crate) control_rounds: JoinSet<(
        crate::control_actor::ScheduledControlRound,
        Result<Option<crate::control_sync::ControlRoundOutcome>, RuntimeError>,
    )>,
    pub(crate) control_queue: crate::control_actor::ControlRoundQueue,
    pub(crate) synchronized_control_peers: BTreeSet<ma2a_core::EndpointId>,
    #[cfg(test)]
    pub(crate) control_schedule_events:
        Arc<std::sync::Mutex<Vec<crate::control_sync::ControlRoundTrigger>>>,
    cancellation: CancellationToken,
}

impl Actor {
    #[allow(
        clippy::too_many_arguments,
        reason = "the actor owns each independently constructed runtime subsystem"
    )]
    pub(crate) fn new(
        state: RuntimeStatus,
        endpoint: RuntimeEndpoint,
        store: StoreClient,
        enrollment_calls: mpsc::Receiver<EnrollmentCall>,
        control_calls: mpsc::Receiver<ControlCall>,
        lookup: SpaceAddressLookup,
        clock: Arc<dyn crate::RuntimeClock>,
    ) -> (Self, RuntimeHandle, CancellationToken) {
        let (command_sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        #[cfg(test)]
        let control_schedule_events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let (relay_observation_sender, relay_observations) = mpsc::channel(COMMAND_CAPACITY);
        let relay_observer = endpoint.spawn_relay_observer(relay_observation_sender);
        let handle = RuntimeHandle::new(
            command_sender,
            events.clone(),
            #[cfg(test)]
            Arc::clone(&control_schedule_events),
        );
        let actor = Self {
            state,
            endpoint,
            store,
            commands,
            events,
            clock,
            enrollment_calls,
            control_calls,
            lookup,
            relay_observations,
            relay_observer,
            control_rounds: JoinSet::new(),
            control_queue: crate::control_actor::ControlRoundQueue::default(),
            synchronized_control_peers: BTreeSet::new(),
            #[cfg(test)]
            control_schedule_events,
            cancellation: cancellation.child_token(),
        };
        (actor, handle, cancellation)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "single select loop keeps actor command ordering explicit"
    )]
    pub(crate) async fn run(mut self) -> Result<ShutdownAck, RuntimeError> {
        let _receiver_count = self.events.send(RuntimeEvent::ready(self.state.revision));
        self.schedule_control_round(crate::control_sync::ControlRoundTrigger::Startup, None);
        let control_period = crate::control_actor::control_period(self.state.endpoint_id);
        let mut periodic =
            tokio::time::interval_at(tokio::time::Instant::now() + control_period, control_period);
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
                    Some(Command::CreateEnrollmentInvite { creation, reply }) => {
                        let result = match self.clock.now_ms()
                            .ok()
                            .and_then(|value| u64::try_from(value).ok())
                            .and_then(|now_ms| creation.issue_at(now_ms).ok())
                        {
                            Some(creation) => self.store.create_enrollment_invite(
                                creation, self.state.endpoint_id, self.state.endpoint_addr.clone(),
                            ).await.map_err(|_| EnrollmentError::internal()),
                            None => Err(EnrollmentError::internal()),
                        };
                        let _unsent = reply.send(result);
                    }
                    Some(Command::RedeemEnrollment { attempt, reply }) => {
                        let result = self.redeem_enrollment(*attempt).await;
                        let _unsent = reply.send(result);
                    }
                    Some(Command::CancelEnrollmentInvite { invitation_id, reply }) => {
                        let result = self.store.cancel_enrollment_invite(invitation_id).await
                            .map(|revision| { self.state.revision = revision; })
                            .map_err(|_| EnrollmentError::internal());
                        let _unsent = reply.send(result);
                    }
                    Some(Command::AdoptRevision { revision, reply }) => {
                        self.state.revision = self.state.revision.max(revision);
                        let _unsent = reply.send(self.state.revision);
                    }
                    Some(Command::ControlSyncStatus { peer, reply }) => {
                        let _unsent = reply.send(self.synchronized_control_peers.contains(&peer));
                    }
                    Some(Command::SyncControl { peer, reply }) => {
                        let scope = peer.map_or_else(
                            crate::control_sync::ControlRoundScope::all,
                            crate::control_sync::ControlRoundScope::peer,
                        );
                        self.schedule_control_round(
                            crate::control_sync::ControlRoundTrigger::Explicit(scope),
                            Some(reply),
                        );
                    }
                    Some(Command::AdvanceOwnedSpace { update, reply }) => {
                        let result = match self
                            .store
                            .advance_owned_space(update, self.state.endpoint_id)
                            .await
                        {
                            Ok((revision, memberships)) => {
                                self.state.revision = revision;
                                self.state.memberships = memberships;
                                self.endpoint
                                    .set_control_enabled(!self.state.memberships.is_empty());
                                match self.refresh_control_lookup().await {
                                    Ok(()) => match self.refresh_relay_candidates().await {
                                        Ok(_) => {
                                            self.schedule_control_round(
                                                crate::control_sync::ControlRoundTrigger::ManifestAdvanced,
                                                None,
                                            );
                                            Ok(revision)
                                        }
                                        Err(error) => Err(error),
                                    },
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        };
                        let _unsent = reply.send(result);
                    }
                    Some(Command::PublishAddress(reply)) => {
                        let result = self.publish_local_address().await;
                        let _unsent = reply.send(result);
                    }
                    Some(Command::PublishRelayAdvertisements { config, expires_at_ms, reply }) => {
                        let result = self.publish_local_relay(config, expires_at_ms).await;
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
                },
                call = self.enrollment_calls.recv() => if let Some(call) = call {
                    self.handle_enrollment_call(call).await;
                },
                call = self.control_calls.recv() => if let Some(call) = call {
                    self.handle_control_call(call).await;
                },
                observation = self.relay_observations.recv() => if let Some(observation) = observation {
                    self.observe_iroh_relay(observation).await?;
                },
                joined = self.control_rounds.join_next(), if !self.control_rounds.is_empty() => {
                    if let Some(result) = joined {
                        self.finish_control_round(result).await;
                    }
                },
                _ = periodic.tick() => {
                    self.refresh_relay_candidates().await?;
                    self.schedule_control_round(
                        crate::control_sync::ControlRoundTrigger::Periodic, None,
                    );
                },
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
        self.synchronized_control_peers.clear();
        self.endpoint
            .set_control_enabled(!self.state.memberships.is_empty());
        self.refresh_control_lookup().await?;
        self.schedule_control_round(
            crate::control_sync::ControlRoundTrigger::ManifestAdvanced,
            None,
        );
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
        self.control_rounds.shutdown().await;
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
