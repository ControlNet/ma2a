use std::{path::PathBuf, sync::Arc};

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentMessage, AgentSubmit, MAX_LOG_LIMIT, MessageRole, ProcessSpec, ProcessStart,
        ServiceError,
    },
};
use ma2a_store::{AgentDefinition, Repository, StoreConfig};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AgentManager;
use crate::{
    clock::SystemClock,
    processes::ProcessManager,
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

/// An Agent manager over a real Store that holds the definition of the reserved name `blobs`,
/// stored directly, as a Runtime that did not refuse the name did.
struct Reserved {
    dir: PathBuf,
    config: StoreConfig,
    store: StoreClient,
    store_task: tokio::task::JoinHandle<()>,
    manager: AgentManager,
    id: AgentId,
}

impl Reserved {
    async fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let dir = std::env::temp_dir().join(format!("ma2a-reserved-{}", u64::from_le_bytes(nonce)));
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
        let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
        let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
        let store = StoreClient::new(sender);
        let local = store.initialize().await?.endpoint_id;
        let id = AgentId::new("blobs")?;
        let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
        let definition = AgentDefinition {
            id: id.clone(),
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
        Ok(Self {
            dir,
            config,
            store,
            store_task,
            manager,
            id,
        })
    }

    async fn submit(&self) -> Result<bool, ServiceError> {
        let message = AgentMessage::text(MessageRole::User, "hello")
            .map_err(|_| ServiceError::InvalidInput)?;
        let binding = (
            self.manager.0.local,
            RequestId::random().map_err(|_| ServiceError::Internal)?,
        );
        let submit = AgentSubmit::new(self.id.clone(), message, None);
        self.manager
            .submit(binding, &submit)
            .await
            .map(|(_, existed)| existed)
    }

    async fn finish(self) -> TestResult {
        drop(self.manager);
        self.store.stop().await?;
        self.store_task.await?;
        let _cleanup = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

#[tokio::test]
async fn a_reserved_agent_defined_before_the_refusal_is_never_started() -> TestResult {
    // Given
    let reserved = Reserved::new().await?;

    // When
    let started = reserved.manager.start(&reserved.id).await;

    // Then
    let enabled = reserved
        .manager
        .agent(&reserved.id)
        .await?
        .map(|record| record.enabled);
    reserved.finish().await?;
    assert_eq!(started.err(), Some(ServiceError::InvalidInput));
    assert_eq!(enabled, Some(false));
    Ok(())
}

#[tokio::test]
async fn a_reserved_agent_left_enabled_is_stopped_by_recovery() -> TestResult {
    // Given: the definition was started before its name was refused, and holds a queued task.
    let reserved = Reserved::new().await?;
    Repository::open(&reserved.config)?.set_agent_enabled(&reserved.id, (true, 2))?;
    assert_eq!(reserved.submit().await, Ok(false));

    // When
    reserved.manager.reconcile().await;

    // Then: it is stopped as an operator stop would, so it takes no new task.
    let submitted = reserved.submit().await;
    let enabled = reserved
        .manager
        .agent(&reserved.id)
        .await?
        .map(|record| record.enabled);
    let live = Repository::open(&reserved.config)?.live_agent_tasks()?;
    reserved.finish().await?;
    assert_eq!(submitted, Err(ServiceError::AgentUnavailable));
    assert_eq!(enabled, Some(false));
    assert!(live.is_empty(), "the queued task ends as on a stop");
    Ok(())
}

#[test]
fn names_the_runtime_or_windows_keeps_are_reserved() {
    for name in ["blobs", "con", "nul", "aux", "prn", "com1", "lpt9"] {
        assert!(super::reserved_name(name), "{name}");
    }
    for name in ["lead", "blob", "console", "com", "com10", "nul-agent"] {
        assert!(!super::reserved_name(name), "{name}");
    }
}
