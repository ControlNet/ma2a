use std::collections::BTreeSet;

use ma2a_net::RuntimeEndpoint;
use ma2a_store::StoreConfig;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    actor::{Actor, RuntimeHandle, ShutdownAck},
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
        let backend = tokio::task::spawn_blocking(move || StoreBackend::open(&config)).await??;
        let (store_sender, store_receiver) = mpsc::channel(STORE_CAPACITY);
        let store = StoreClient::new(store_sender);
        let mut tasks = JoinSet::new();
        tasks.spawn_blocking(move || {
            backend.run(store_receiver);
            Ok(TaskExit::Store)
        });
        let identity = store.initialize().await?;
        let boot_id = boot_id()?;
        let boot_revision = store.begin_boot(boot_id).await?;
        let endpoint = RuntimeEndpoint::bind(identity.secret).await?;
        if endpoint.endpoint_id() != identity.endpoint_id {
            return Err(RuntimeError::new(RuntimeErrorKind::IdentityMismatch));
        }
        let mut state = RuntimeStatus {
            endpoint_id: identity.endpoint_id,
            endpoint_addr: endpoint.endpoint_addr(),
            boot_id,
            revision: boot_revision,
            memberships: BTreeSet::default(),
            ready: true,
            connectivity: Connectivity::DIRECT_ONLY,
        };
        state.revision = store.observe(&state).await?.max(boot_revision);
        let (actor, handle, cancellation) = Actor::new(state, endpoint, store);
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
