use std::sync::Arc;

use ma2a_net::{EndpointBindOptions, RuntimeEndpoint, SpaceAddressLookup};
use ma2a_store::StoreConfig;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    actor::{Actor, RuntimeHandle, ShutdownAck},
    clock::{RuntimeAddressLookupClock, SystemClock},
    error::{RuntimeError, RuntimeErrorKind},
    state::{Connectivity, RuntimeStatus, ShutdownReport},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

enum TaskExit {
    Actor(ShutdownAck),
    Store,
    Services,
}

/// Admitted inbound service streams queued for the stream host; beyond this, callers are
/// told the service is unavailable rather than buffered.
const SERVICE_QUEUE: usize = 64;

/// One running Runtime actor and all tasks it owns.
#[derive(Debug)]
pub struct Runtime {
    handle: RuntimeHandle,
    connections: crate::RuntimeConnections,
    cancellation: CancellationToken,
    tasks: JoinSet<Result<TaskExit, RuntimeError>>,
}

impl Runtime {
    /// Opens protected state, binds one Iroh Endpoint, and reaches ready state with zero Spaces.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] on any fail-closed storage, identity, or Endpoint failure.
    pub async fn start(config: StoreConfig) -> Result<Self, RuntimeError> {
        Self::start_with_clock(config, Arc::new(SystemClock)).await
    }

    /// Opens a Runtime with an injected authoritative security clock.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] on storage, identity, clock, or Endpoint failure.
    pub async fn start_with_clock(
        config: StoreConfig,
        clock: Arc<dyn crate::RuntimeClock>,
    ) -> Result<Self, RuntimeError> {
        // Initialization owns sizeable Runtime projections across I/O awaits.
        // Keep callers' future size independent of those internal projections.
        Box::pin(Self::initialize(config, clock, None)).await
    }

    /// Opens a Runtime that also starts new Processes with `hosting`.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] on any fail-closed storage, identity, or Endpoint failure.
    pub async fn start_with_processes(
        config: StoreConfig,
        hosting: crate::ProcessHosting,
    ) -> Result<Self, RuntimeError> {
        Box::pin(Self::initialize(
            config,
            Arc::new(SystemClock),
            Some(hosting),
        ))
        .await
    }

    async fn initialize(
        config: StoreConfig,
        clock: Arc<dyn crate::RuntimeClock>,
        hosting: Option<crate::ProcessHosting>,
    ) -> Result<Self, RuntimeError> {
        let state_dir = config.state_dir().to_path_buf();
        let backend = tokio::task::spawn_blocking(move || StoreBackend::open(&config)).await??;
        let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
        let store = StoreClient::new(store_sender);
        let mut tasks = JoinSet::new();
        tasks.spawn_blocking(move || {
            backend.run(store_receiver);
            Ok(TaskExit::Store)
        });
        let identity = store.initialize().await?;
        let _swept = store.sweep_staging().await?;
        let (enrollment_sender, enrollment_calls) = mpsc::channel(crate::actor::COMMAND_CAPACITY);
        let (control_sender, control_calls) = mpsc::channel(crate::actor::COMMAND_CAPACITY);
        let (echo_sender, echo_calls) = mpsc::channel(crate::actor::COMMAND_CAPACITY);
        let (service_sender, service_calls) = mpsc::channel(SERVICE_QUEUE);
        let lookup = SpaceAddressLookup::with_clock(Arc::new(RuntimeAddressLookupClock::new(
            Arc::clone(&clock),
        )));
        let now_ms = u64::try_from(clock.now_ms()?)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let lookup_state = store
            .load_control_lookup(identity.endpoint_id, now_ms)
            .await?;
        crate::control_sync::install_lookup(&lookup, lookup_state)?;
        let relay_map = store.load_relay_map(identity.endpoint_id, now_ms).await?;
        let options = EndpointBindOptions::new(enrollment_sender, identity.bind_port)
            .with_control(control_sender, !identity.memberships.is_empty())
            .with_echo(echo_sender)
            .with_services(service_sender)
            .with_relay_map(relay_map.clone());
        let endpoint =
            RuntimeEndpoint::bind_with_lookup(identity.secret, lookup.clone(), options).await?;
        if endpoint.endpoint_id() != identity.endpoint_id {
            return Err(RuntimeError::new(RuntimeErrorKind::IdentityMismatch));
        }
        if identity.bind_port.is_none() {
            let port = endpoint.bind_port()?;
            store.set_endpoint_bind_port(port).await?;
        }
        let connections = crate::RuntimeConnections::new(&endpoint.connection_manager());
        let echo_metrics = endpoint.echo_metrics();
        let boot_id = boot_id()?;
        let boot_revision = store.begin_boot(boot_id).await?;
        let relay_observation_revision = store.record_relay_observations(Vec::new()).await?;
        let endpoint_data = endpoint
            .endpoint_data()
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let state = RuntimeStatus {
            endpoint_id: identity.endpoint_id,
            endpoint_addr: endpoint.endpoint_addr(),
            endpoint_data,
            boot_id,
            revision: boot_revision.max(relay_observation_revision),
            memberships: identity.memberships,
            ready: true,
            connectivity: Connectivity::DIRECT_ONLY,
            direct_reachable: false,
            relay: crate::reachability::RelayReachabilityState::new(relay_map),
        };
        let process_clock = Arc::clone(&clock);
        let (mut actor, handle, cancellation) = Actor::new(
            state,
            endpoint,
            store.clone(),
            enrollment_calls,
            control_calls,
            echo_calls,
            echo_metrics,
            lookup,
            clock,
            None,
        );
        if let Err(error) = actor.initialize().await {
            let cleanup = actor.finish(false).await;
            drop(handle);
            while let Some(joined) = tasks.join_next().await {
                if let Err(join_error) = joined {
                    eprintln!("Runtime initialization task cleanup failed: {join_error}");
                }
            }
            if let Err(cleanup_error) = cleanup {
                eprintln!("Runtime initialization cleanup failed: {cleanup_error}");
            }
            return Err(error);
        }
        tasks.spawn(async move { actor.run().await.map(TaskExit::Actor) });
        let host = service_host(
            (identity.endpoint_id, store, process_clock),
            (&state_dir, hosting),
            (&cancellation, &handle),
        );
        spawn_service_host(&mut tasks, service_calls, host);
        Ok(Self {
            handle,
            connections,
            cancellation,
            tasks,
        })
    }

    /// Returns a cloneable bounded actor handle.
    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    /// Returns bounded observational Iroh connection telemetry.
    pub const fn connections(&self) -> &crate::RuntimeConnections {
        &self.connections
    }

    /// Gracefully closes Iroh and joins the actor plus blocking store owner.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] if any owned task or persistence step fails.
    pub async fn shutdown(mut self) -> Result<ShutdownReport, RuntimeError> {
        let ack = self.handle.shutdown().await;
        self.cancellation.cancel();
        drop(self.handle);
        let mut joined_tasks = 0_usize;
        let mut actor_ack = None;
        let mut failure = None;
        while let Some(joined) = self.tasks.join_next().await {
            joined_tasks = joined_tasks
                .checked_add(1)
                .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Shutdown))?;
            match joined.map_err(RuntimeError::from).and_then(|exit| exit) {
                Ok(TaskExit::Actor(exit)) => actor_ack = Some(exit),
                Ok(TaskExit::Store | TaskExit::Services) => {}
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let ack = ack?;
        if joined_tasks != 3 || actor_ack != Some(ack) {
            return Err(RuntimeError::new(RuntimeErrorKind::Shutdown));
        }
        Ok(ShutdownReport {
            joined_tasks,
            endpoint_closed: ack.endpoint_closed,
            revision: ack.revision,
        })
    }
}

/// Everything the inbound service host and the Process manager share.
fn service_host(
    (local, store, clock): (
        ma2a_core::EndpointId,
        StoreClient,
        Arc<dyn crate::RuntimeClock>,
    ),
    (state_dir, hosting): (&std::path::Path, Option<crate::ProcessHosting>),
    (cancellation, handle): (&CancellationToken, &crate::RuntimeHandle),
) -> crate::stream_host::HostContext {
    let processes = crate::processes::ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (state_dir, hosting),
        cancellation.child_token(),
    );
    processes.fence_startup();
    handle.install_processes(processes.clone());
    let hosted = crate::hosting::HostedServices::new((local, store.clone(), Arc::clone(&clock)));
    handle.install_hosted(hosted.clone());
    let agents = crate::agents::AgentManager::new(
        (local, store.clone(), clock),
        (state_dir, processes.clone()),
        cancellation.child_token(),
    );
    handle.install_agents(agents.clone());
    crate::stream_host::HostContext {
        local,
        store,
        processes,
        agents,
        hosted,
        shutdown: cancellation.child_token(),
        operations: handle.operations().clone(),
        authorizations: crate::stream_host::Authorizations::default(),
    }
}

/// Serves admitted inbound service streams until the Runtime's cancellation, which the
/// host's own token descends from, so shutdown reaches every streaming operation.
fn spawn_service_host(
    tasks: &mut JoinSet<Result<TaskExit, RuntimeError>>,
    calls: mpsc::Receiver<ma2a_net::ServiceCall>,
    host: crate::stream_host::HostContext,
) {
    tasks.spawn(async move {
        crate::stream_host::run(calls, host).await;
        Ok(TaskExit::Services)
    });
}

fn boot_id() -> Result<[u8; 16], RuntimeError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RuntimeError::new(RuntimeErrorKind::Random))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
