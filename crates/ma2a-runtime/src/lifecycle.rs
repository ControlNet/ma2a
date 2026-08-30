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
}

/// One running Runtime actor and all tasks it owns.
#[derive(Debug)]
pub struct Runtime {
    handle: RuntimeHandle,
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
        let backend = tokio::task::spawn_blocking(move || StoreBackend::open(&config)).await??;
        let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
        let store = StoreClient::new(store_sender);
        let mut tasks = JoinSet::new();
        tasks.spawn_blocking(move || {
            backend.run(store_receiver);
            Ok(TaskExit::Store)
        });
        let identity = store.initialize().await?;
        let (enrollment_sender, enrollment_calls) = mpsc::channel(crate::actor::COMMAND_CAPACITY);
        let (control_sender, control_calls) = mpsc::channel(crate::actor::COMMAND_CAPACITY);
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
        let boot_id = boot_id()?;
        let boot_revision = store.begin_boot(boot_id).await?;
        let relay_observation_revision = store.record_relay_observations(Vec::new()).await?;
        let endpoint_data = endpoint
            .endpoint_data()
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
        let mut state = RuntimeStatus {
            endpoint_id: identity.endpoint_id,
            endpoint_addr: endpoint.endpoint_addr(),
            endpoint_data,
            boot_id,
            revision: boot_revision.max(relay_observation_revision),
            memberships: identity.memberships,
            ready: true,
            connectivity: Connectivity::DIRECT_ONLY,
            relay: crate::reachability::RelayReachabilityState::new(relay_map),
        };
        state.revision = store.observe(&state).await?.max(boot_revision);
        let (actor, handle, cancellation) = Actor::new(
            state,
            endpoint,
            store,
            enrollment_calls,
            control_calls,
            lookup,
            clock,
        );
        tasks.spawn(async move { actor.run().await.map(TaskExit::Actor) });
        Ok(Self {
            handle,
            cancellation,
            tasks,
        })
    }

    /// Returns a cloneable bounded actor handle.
    pub fn handle(&self) -> RuntimeHandle {
        self.handle.clone()
    }

    /// Gracefully closes Iroh and joins the actor plus blocking store owner.
    ///
    /// # Errors
    /// Returns [`RuntimeError`] if any owned task or persistence step fails.
    pub async fn shutdown(mut self) -> Result<ShutdownReport, RuntimeError> {
        let ack = self.handle.shutdown().await?;
        self.cancellation.cancel();
        drop(self.handle);
        let mut joined_tasks = 0_usize;
        let mut actor_ack = None;
        while let Some(joined) = self.tasks.join_next().await {
            joined_tasks = joined_tasks
                .checked_add(1)
                .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Shutdown))?;
            match joined?? {
                TaskExit::Actor(exit) => actor_ack = Some(exit),
                TaskExit::Store => {}
            }
        }
        if joined_tasks != 2 || actor_ack != Some(ack) {
            return Err(RuntimeError::new(RuntimeErrorKind::Shutdown));
        }
        Ok(ShutdownReport {
            joined_tasks,
            endpoint_closed: ack.endpoint_closed,
            revision: ack.revision,
        })
    }
}

fn boot_id() -> Result<[u8; 16], RuntimeError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| RuntimeError::new(RuntimeErrorKind::Random))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use ma2a_store::{Repository, StoreConfig};

    use super::{Runtime, TaskExit};

    static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

    struct TempState(PathBuf);

    impl TempState {
        fn new() -> Result<Self, Box<dyn Error + Send + Sync>> {
            let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ma2a-runtime-cancel-{}-{serial}",
                std::process::id()
            ));
            fs::create_dir(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
            }
            Ok(Self(path))
        }
    }

    impl Drop for TempState {
        fn drop(&mut self) {
            let _cleanup_result = fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_reports_the_persisted_observation_revision()
    -> Result<(), Box<dyn Error + Send + Sync>> {
        // Given
        let state = TempState::new()?;
        let config = StoreConfig::new(&state.0);
        let Runtime {
            handle,
            cancellation,
            mut tasks,
        } = Runtime::start(config.clone()).await?;

        // When
        cancellation.cancel();
        drop(handle);
        let mut actor_revision = None;
        while let Some(joined) = tasks.join_next().await {
            if let TaskExit::Actor(ack) = joined?? {
                actor_revision = Some(ack.revision);
            }
        }
        let persisted_revision = Repository::open(&config)?.runtime_metadata()?.revision();

        // Then
        assert_eq!(actor_revision, Some(persisted_revision));
        Ok(())
    }
}
