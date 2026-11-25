//! An Agent manager over a real Store holding one running task, stopped on demand.

use std::{path::PathBuf, sync::Arc, time::Duration};

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentMessage, AgentTaskId, AgentTaskReason, MAX_LOG_LIMIT, MessageRole, ProcessId,
        ProcessSpec, ProcessStart, ServiceError,
    },
};
use ma2a_store::{
    AgentDefinition, AgentOutput, AgentOutputEffect, AgentTaskCreation, NewAgentTask, Repository,
    StoreConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AgentManager;
use crate::{
    clock::SystemClock,
    processes::ProcessManager,
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Bit of the `agent` tool family.
const AGENT_TOOLS: u8 = 32;

struct Live {
    dir: PathBuf,
    config: StoreConfig,
    store: StoreClient,
    store_task: tokio::task::JoinHandle<()>,
    manager: AgentManager,
    shutdown: CancellationToken,
    agent: AgentId,
    generation: u64,
    task: AgentTaskId,
}

impl Live {
    async fn new() -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let dir = std::env::temp_dir().join(format!("ma2a-live-{}", u64::from_le_bytes(nonce)));
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
        let agent = AgentId::new("helper")?;
        let (generation, task) = running(&config, (&agent, local))?;
        let shutdown = CancellationToken::new();
        let processes = ProcessManager::new(
            (local, store.clone(), Arc::new(SystemClock)),
            (&dir, None),
            shutdown.clone(),
        );
        let manager = AgentManager::new(
            (local, store.clone(), Arc::new(SystemClock)),
            (&dir, processes),
            shutdown.clone(),
        );
        Ok(Self {
            dir,
            config,
            store,
            store_task,
            manager,
            shutdown,
            agent,
            generation,
            task,
        })
    }

    /// Records one pending tool call of the running task, as its instance's output.
    fn call(&self, (call, name, args): (&str, &str, &serde_json::Value)) -> TestResult {
        let output = AgentOutput {
            agent: self.agent.clone(),
            generation: self.generation,
            cursor: 100,
            at_ms: 3_000,
        };
        let effect = AgentOutputEffect::ToolCall {
            task: self.task,
            call: call.to_owned(),
            name: name.to_owned(),
            request: serde_json::to_vec(args)?,
            rejection: b"{\"error\":{\"code\":\"concurrency_exceeded\"}}".to_vec(),
        };
        Repository::open(&self.config)?.apply_agent_output(&output, effect)?;
        Ok(())
    }

    async fn finish(self) -> TestResult {
        drop(self.manager);
        self.store.stop().await?;
        self.store_task.await?;
        let _cleanup = std::fs::remove_dir_all(&self.dir);
        Ok(())
    }
}

/// Defines and starts `agent` with a ready instance, and gives it one running task.
fn running(
    config: &StoreConfig,
    (agent, creator): (&AgentId, ma2a_core::EndpointId),
) -> TestResult<(u64, AgentTaskId)> {
    let mut repository = Repository::open(config)?;
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let definition = AgentDefinition {
        id: agent.clone(),
        description: String::new(),
        start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
        tools: AGENT_TOOLS,
        max_tasks: 1,
    };
    repository.define_agent(&definition, 1)?;
    repository.set_agent_enabled(agent, (true, 2))?;
    let generation = repository
        .begin_agent_instance(agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?
        .generation;
    repository.bind_agent_process((agent, generation), ProcessId::from_bytes([4; 16]))?;
    let ready = AgentOutput {
        agent: agent.clone(),
        generation,
        cursor: 1,
        at_ms: 5,
    };
    repository.apply_agent_output(&ready, AgentOutputEffect::Ready)?;
    let task = NewAgentTask {
        id: AgentTaskId::random()?,
        agent: agent.clone(),
        binding: (creator, RequestId::random()?),
        fingerprint: [6; 32],
        from_agent: None,
        input: AgentMessage::text(MessageRole::User, "work")?,
        created_ms: 7,
    };
    let AgentTaskCreation::Created(record) = repository.create_agent_task(&task)?.into_value()
    else {
        return Err("the task was not created".into());
    };
    repository.assign_agent_tasks(agent, 8)?;
    Ok((generation, record.summary.id))
}

#[tokio::test]
async fn a_cancellation_waiting_for_its_agent_ends_when_the_operation_stops() -> TestResult {
    // Given: a running task whose Agent never confirms a cancellation.
    let live = Live::new().await?;
    let stop = CancellationToken::new();
    let stopper = stop.clone();
    let _stopping = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stopper.cancel();
    });

    // When: the operation is stopped while the cancellation waits.
    let request = (live.task, AgentTaskReason::Requested, &stop);
    let cancelled = tokio::time::timeout(
        Duration::from_secs(5),
        live.manager.cancel((None, None), request),
    )
    .await;

    // Then: the answer is at once, and says the end is not known.
    live.finish().await?;
    assert_eq!(
        cancelled
            .map_err(|_| "the cancellation outlived its operation")?
            .err(),
        Some(ServiceError::OutcomeUnknown)
    );
    Ok(())
}
