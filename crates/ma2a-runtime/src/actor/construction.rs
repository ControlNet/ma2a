use std::sync::Arc;

use ma2a_net::{
    ControlCall, EchoCall, EchoMetrics, EnrollmentCall, RuntimeEndpoint, SpaceAddressLookup,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Actor, COMMAND_CAPACITY, RuntimeHandle};
use crate::{RuntimeConnections, state::RuntimeStatus, store::StoreClient};

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
        echo_calls: mpsc::Receiver<EchoCall>,
        echo_metrics: EchoMetrics,
        lookup: SpaceAddressLookup,
        clock: Arc<dyn crate::RuntimeClock>,
        private_relay_server: Option<ma2a_net::PrivateRelayServer>,
    ) -> (Self, RuntimeHandle, CancellationToken) {
        let (command_sender, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (events, _) = tokio::sync::broadcast::channel(COMMAND_CAPACITY);
        #[cfg(test)]
        let control_schedule_events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let cancellation = CancellationToken::new();
        let echo_audit = crate::echo_audit::EchoAuditLog::default();
        let (relay_observation_sender, relay_observations) = mpsc::channel(COMMAND_CAPACITY);
        let relay_observer = endpoint.spawn_relay_observer(relay_observation_sender);
        let connections = RuntimeConnections::new(&endpoint.connection_manager());
        let handle = RuntimeHandle::new(
            command_sender,
            store.clone(),
            events.clone(),
            echo_audit.clone(),
            echo_metrics.clone(),
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
            control_tasks: tokio::task::JoinSet::new(),
            echo_calls,
            echo_tasks: tokio::task::JoinSet::new(),
            echo_audit,
            echo_metrics,
            connections,
            lookup,
            relay_observations,
            relay_observer,
            private_relay_server,
            control_rounds: tokio::task::JoinSet::new(),
            control_queue: crate::control_actor::ControlRoundQueue::default(),
            synchronized_control_peers: std::collections::BTreeSet::new(),
            control_round_history: std::collections::VecDeque::new(),
            #[cfg(test)]
            control_schedule_events,
            cancellation: cancellation.child_token(),
        };
        (actor, handle, cancellation)
    }
}
