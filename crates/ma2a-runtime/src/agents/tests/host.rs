//! Agent discovery over a Store whose authorization reads fail.

use std::sync::Arc;

use ma2a_core::service::{AgentId, MAX_LOG_LIMIT, ProcessSpec, ProcessStart, ServiceError};
use ma2a_store::{AgentDefinition, Repository, StoreConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AgentManager;
use crate::{
    clock::SystemClock,
    processes::ProcessManager,
    store::{STORE_CAPACITY, ServiceStoreCommand, StoreBackend, StoreClient, StoreCommand},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn discovery_fails_when_an_agent_cannot_be_authorized() -> TestResult {
    // Given: one defined Agent, and a Store that answers everything but authorization.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-discover-{}", u64::from_le_bytes(nonce)));
    std::fs::create_dir(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    ma2a_store::protect_private_directory(&dir, "test state directory")?;
    let config = StoreConfig::new(&dir);
    let backend = StoreBackend::open(&config)?;
    let (forward, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let (sender, mut commands) = mpsc::channel(STORE_CAPACITY);
    let filter = tokio::spawn(async move {
        while let Some(command) = commands.recv().await {
            // An authorization whose reply is dropped is answered `unavailable`.
            if !matches!(
                command,
                StoreCommand::Services(ServiceStoreCommand::Authorize { .. })
            ) && forward.send(command).await.is_err()
            {
                return;
            }
        }
    });
    let store = StoreClient::new(sender);
    let local = store.initialize().await?.endpoint_id;
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let definition = AgentDefinition {
        id: AgentId::new("helper")?,
        description: String::new(),
        start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
        tools: 0,
        max_tasks: 1,
    };
    Repository::open(&config)?.define_agent(&definition, 1)?;
    let shutdown = CancellationToken::new();
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (&dir, None),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (&dir, processes),
        shutdown,
    );
    let peer = iroh::SecretKey::generate().public().into();

    // When
    let discovered = manager.discover(peer, None).await;

    // Then: a failed read fails the page instead of hiding the Agent.
    drop(manager);
    store.stop().await?;
    drop(store);
    filter.await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);
    assert_eq!(discovered.err(), Some(ServiceError::Unavailable));
    Ok(())
}
