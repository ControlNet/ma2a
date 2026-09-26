use std::{collections::BTreeSet, sync::Arc};

use ma2a_net::{
    ControlCall, EchoCall, EchoMetrics, EnrollmentCall, IrohRelayObservation, RuntimeEndpoint,
    SpaceAddressLookup,
};
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

use crate::state::{RuntimeEvent, RuntimeStatus};
use crate::{error::RuntimeError, store::StoreClient};

#[cfg(test)]
mod background_reconciliation_test;
mod command;
mod construction;
#[cfg(test)]
mod control_completion_test;
mod echo;
#[cfg(test)]
mod effective_data_test;
#[cfg(test)]
mod enrollment_completion_test;
mod handle;
mod handle_relay;
mod invite;
mod local_control;
pub(crate) mod maintenance;
mod membership;
#[cfg(test)]
mod owner_invariant_test;
#[cfg(test)]
mod partial_relay_publication_test;
#[cfg(test)]
mod private_server_test;
mod relay_configuration;
#[cfg(test)]
mod relay_lifecycle_test;
pub(crate) mod relay_server;
mod run;
mod shutdown;
mod snapshot;
pub(crate) use command::Command;
pub use handle::RuntimeHandle;
pub(crate) use handle_relay::RelayRuntimeStatus;
pub(crate) use shutdown::ShutdownAck;

pub(crate) const COMMAND_CAPACITY: usize = 32;

pub(crate) struct Actor {
    pub(crate) state: RuntimeStatus,
    pub(crate) endpoint: RuntimeEndpoint,
    pub(crate) store: StoreClient,
    commands: mpsc::Receiver<Command>,
    pub(crate) events: broadcast::Sender<RuntimeEvent>,
    pub(crate) clock: Arc<dyn crate::RuntimeClock>,
    enrollment_calls: mpsc::Receiver<EnrollmentCall>,
    pub(crate) control_calls: mpsc::Receiver<ControlCall>,
    pub(crate) control_tasks: JoinSet<crate::control_actor::inbound::InboundControlCompletion>,
    echo_calls: mpsc::Receiver<EchoCall>,
    echo_tasks: JoinSet<echo::EchoTaskCompletion>,
    pub(crate) echo_audit: crate::echo_audit::EchoAuditLog,
    pub(crate) echo_metrics: EchoMetrics,
    pub(crate) connections: crate::RuntimeConnections,
    pub(crate) lookup: SpaceAddressLookup,
    relay_observations: mpsc::Receiver<IrohRelayObservation>,
    relay_observer: tokio::task::JoinHandle<()>,
    private_relay_server: Option<ma2a_net::PrivateRelayServer>,
    pub(crate) maintenance: maintenance::Maintenance,
    pub(crate) control_rounds: JoinSet<(
        crate::control_actor::ScheduledControlRound,
        Result<
            Option<crate::control_sync::ControlRoundOutcome>,
            crate::control_sync::ControlFailure,
        >,
    )>,
    pub(crate) control_queue: crate::control_actor::ControlRoundQueue,
    pub(crate) synchronized_control_peers: BTreeSet<ma2a_core::EndpointId>,
    pub(crate) control_round_history: std::collections::VecDeque<crate::api::ControlRoundView>,
    #[cfg(test)]
    pub(crate) control_schedule_events:
        Arc<std::sync::Mutex<Vec<crate::control_sync::ControlRoundTrigger>>>,
    cancellation: CancellationToken,
}

impl Actor {
    pub(crate) fn absorb_background(
        step: &str,
        result: Result<(), RuntimeError>,
    ) -> Result<(), RuntimeError> {
        match result {
            Err(error) if error.is_retryable_background() => {
                eprintln!("runtime background step will retry: {step}: {error}");
                Ok(())
            }
            other => other,
        }
    }

    pub(crate) async fn initialize(&mut self) -> Result<(), RuntimeError> {
        let configured = self.reconcile_relay_configuration().await;
        Self::absorb_background("initial private relay role", configured)?;
        self.refresh_local_control_publications().await?;
        self.state.revision = self
            .store
            .observe(&self.state)
            .await?
            .max(self.state.revision);
        Ok(())
    }
}
