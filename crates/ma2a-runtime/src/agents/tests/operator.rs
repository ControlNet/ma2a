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
            .map(|((_, existed), _)| existed)
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

#[tokio::test]
async fn a_reserved_agent_left_enabled_never_begins_an_instance() -> TestResult {
    // Given: the definition was started before its name was refused.
    let reserved = Reserved::new().await?;
    Repository::open(&reserved.config)?.set_agent_enabled(&reserved.id, (true, 2))?;

    // When
    reserved.manager.ensure_instance(&reserved.id).await;

    // Then
    let instance = reserved
        .manager
        .agent(&reserved.id)
        .await?
        .and_then(|record| record.instance);
    reserved.finish().await?;
    assert_eq!(instance, None);
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

fn spec_with_env(
    count: usize,
) -> Result<super::AgentSpec, Box<dyn std::error::Error + Send + Sync>> {
    Ok(super::AgentSpec {
        id: AgentId::new("crowded")?,
        description: String::new(),
        command: ("/bin/true".to_owned(), Vec::new()),
        cwd: None,
        env: (0..count)
            .map(|index| (format!("K{index}"), "v".to_owned()))
            .collect(),
        tools: 0,
        max_tasks: 1,
    })
}

#[tokio::test]
async fn a_definition_whose_instances_could_never_start_is_refused() -> TestResult {
    // Given: every instance adds three adapter variables to the defined environment.
    let reserved = Reserved::new().await?;

    // When
    let crowded = reserved.manager.define(spec_with_env(62)?).await;
    let fitting = reserved.manager.define(spec_with_env(61)?).await;

    // Then: 62 + 3 exceeds the 64 variables a Process may have; 61 + 3 does not.
    reserved.finish().await?;
    assert_eq!(crowded.err(), Some(ServiceError::InvalidInput));
    assert!(fitting.is_ok(), "{fitting:?}");
    Ok(())
}

/// Three arguments of the longest allowed length, and a last one of `len` bytes.
fn spec_with_arg(len: usize) -> Result<super::AgentSpec, Box<dyn std::error::Error + Send + Sync>> {
    let mut args = vec!["x".repeat(ma2a_core::service::MAX_PROCESS_TEXT_LEN); 3];
    args.push("x".repeat(len));
    Ok(super::AgentSpec {
        id: AgentId::new("lengthy")?,
        description: String::new(),
        command: ("/bin/true".to_owned(), args),
        cwd: None,
        env: Vec::new(),
        tools: 0,
        max_tasks: 1,
    })
}

#[tokio::test]
async fn a_definition_whose_instance_start_outgrows_a_header_frame_is_refused() -> TestResult {
    // Given: the longest last argument the defined start itself still encodes within a
    // frame. A text of 256 bytes or more has a 3-byte header, so the start grows byte for
    // byte.
    let reserved = Reserved::new().await?;
    let overhead = spec_with_arg(256)?.definition()?.start.len() - 256;
    let longest = ma2a_core::service::MAX_HEADER_FRAME_LEN - overhead;
    assert!(spec_with_arg(longest)?.definition().is_ok());
    assert!(spec_with_arg(longest + 1)?.definition().is_err());

    // When: every instance adds its working directory, the adapter environment, and a label.
    let crowded = reserved.manager.define(spec_with_arg(longest)?).await;
    let fitting = reserved
        .manager
        .define(spec_with_arg(longest - 1_024)?)
        .await;

    // Then
    reserved.finish().await?;
    assert_eq!(crowded.err(), Some(ServiceError::InvalidInput));
    assert!(fitting.is_ok(), "{fitting:?}");
    Ok(())
}

#[tokio::test]
async fn ensure_instance_never_launches_replacement_before_previous_process_is_terminal()
-> TestResult {
    // Given: an enabled Agent whose previous ended instance still points at a non-terminal
    // Process in `created` state (which cannot yet be terminated directly).
    let reserved = Reserved::new().await?;
    let agent = AgentId::new("guarded")?;
    let spec = super::AgentSpec {
        id: agent.clone(),
        description: String::new(),
        command: ("/bin/true".to_owned(), Vec::new()),
        cwd: None,
        env: Vec::new(),
        tools: 0,
        max_tasks: 1,
    };
    reserved.manager.define(spec).await?;
    let mut repository = Repository::open(&reserved.config)?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let first = repository
        .begin_agent_instance(&agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?;
    let process_id = ma2a_core::service::ProcessId::from_bytes([0x22; 16]);
    let start = ProcessStart::new(
        ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?,
        true,
        MAX_LOG_LIMIT,
    )?
    .to_bytes()?;
    repository.create_process(&ma2a_store::NewProcess {
        id: process_id,
        binding: (reserved.manager.0.local, RequestId::random()?),
        fingerprint: [0x33; 32],
        start: (&start, None),
        secret: [0x44; 32],
        created_ms: 4,
    })?;
    repository.bind_agent_process((&agent, first.generation), process_id)?;
    repository.set_agent_enabled(&agent, (false, 5))?;
    repository.set_agent_enabled(&agent, (true, 6))?;
    drop(repository);

    // When: maintenance asks to ensure a live instance while the previous Process is non-terminal.
    reserved.manager.ensure_instance(&agent).await;

    // Then: no replacement generation is created while the old Process is still non-terminal.
    let current = reserved
        .manager
        .agent(&agent)
        .await?
        .and_then(|record| record.instance)
        .ok_or("instance missing")?;
    reserved.finish().await?;
    assert_eq!(current.generation, first.generation);
    assert!(current.ended_ms.is_some());
    Ok(())
}

#[tokio::test]
async fn unbound_previous_process_blocks_undefine_and_replacement() -> TestResult {
    // Given: an Agent whose generation-1 instance never committed `bind_agent_process`, while
    // its deterministic start RequestId already owns a non-terminal Process in `created` state.
    let reserved = Reserved::new().await?;
    let agent = AgentId::new("unbound-guarded")?;
    let spec = super::AgentSpec {
        id: agent.clone(),
        description: String::new(),
        command: ("/bin/true".to_owned(), Vec::new()),
        cwd: None,
        env: Vec::new(),
        tools: 0,
        max_tasks: 1,
    };
    let ((defined, _), _) = reserved.manager.define(spec).await?;
    let mut repository = Repository::open(&reserved.config)?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let first = repository
        .begin_agent_instance(&agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?;
    let request_id =
        super::super::instance::start_request(&agent, (defined.times.0, first.generation))?;
    let process_id = ma2a_core::service::ProcessId::from_bytes([0x29; 16]);
    let start = ProcessStart::new(
        ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?,
        true,
        MAX_LOG_LIMIT,
    )?
    .to_bytes()?;
    repository.create_process(&ma2a_store::NewProcess {
        id: process_id,
        binding: (reserved.manager.0.local, request_id),
        fingerprint: [0x39; 32],
        start: (&start, None),
        secret: [0x49; 32],
        created_ms: 4,
    })?;
    drop(repository);

    // When: operator stops the Agent before binding, tries to undefine, then re-enables and
    // asks maintenance to ensure a live instance.
    let stop_while_created = reserved.manager.stop(&agent).await;
    let undefine_while_live = reserved.manager.undefine(&agent).await;
    Repository::open(&reserved.config)?.set_agent_enabled(&agent, (true, 6))?;
    reserved.manager.ensure_instance(&agent).await;
    let during_live = reserved
        .manager
        .agent(&agent)
        .await?
        .and_then(|record| record.instance)
        .ok_or("instance missing")?;

    // And once the unbound Process reaches a terminal state, undefine succeeds.
    let second_stop = reserved.manager.stop(&agent).await;
    Repository::open(&reserved.config)?.transition_process(
        process_id,
        ma2a_store::ProcessTransition {
            to: ma2a_core::service::ProcessState::Failed,
            reason: Some(ma2a_core::service::ProcessReason::SpawnFailed),
            termination: None,
            at_ms: 7,
        },
    )?;
    let undefine_after_terminal = reserved.manager.undefine(&agent).await;
    reserved.finish().await?;

    // Then: stop reported Unavailable while committing disabled, and neither undefine nor
    // replacement launch proceeded while the unbound Process lived.
    assert_eq!(stop_while_created.err(), Some(ServiceError::Unavailable));
    assert_eq!(second_stop.err(), Some(ServiceError::Unavailable));
    assert_eq!(undefine_while_live.err(), Some(ServiceError::Unavailable));
    assert_eq!(during_live.generation, first.generation);
    assert!(during_live.ended_ms.is_some());
    assert!(
        undefine_after_terminal.is_ok(),
        "{undefine_after_terminal:?}"
    );
    Ok(())
}

#[cfg(unix)]
async fn launch_unbound_running_agent(
    dir: &std::path::Path,
    (config, store, local): (&StoreConfig, &StoreClient, ma2a_core::EndpointId),
    agent: &AgentId,
) -> Result<ma2a_core::service::ProcessId, Box<dyn std::error::Error + Send + Sync>> {
    use std::time::Duration;

    use ma2a_core::service::ProcessState;

    use crate::processes::ProcessHosting;

    let shutdown = CancellationToken::new();
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (dir, Some(ProcessHosting::in_process())),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (dir, processes.clone()),
        shutdown.clone(),
    );
    let spec = super::AgentSpec {
        id: agent.clone(),
        description: String::new(),
        command: (
            "/bin/sh".to_owned(),
            vec![
                "-c".to_owned(),
                "printf '%s\\n' $$ >> \"$MA2A_AGENT_WORKSPACE/pids\"; exec sleep 60".to_owned(),
            ],
        ),
        cwd: None,
        env: Vec::new(),
        tools: 0,
        max_tasks: 1,
    };
    let ((defined, _), _) = manager.define(spec).await?;
    let mut repository = Repository::open(config)?;
    repository.set_agent_enabled(agent, (true, 2))?;
    let first_instance = repository
        .begin_agent_instance(agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?;
    drop(repository);
    assert_eq!(first_instance.process_id, None);

    let workspace = manager.workspace(agent);
    super::super::private_dir(&workspace)?;
    let start =
        super::super::instance::instance_start(&defined.definition, &workspace.to_string_lossy())?;
    let request_id =
        super::super::instance::start_request(agent, (defined.times.0, first_instance.generation))?;
    let first_process = processes.start_local(request_id, start).await?;
    let pids_path = workspace.join("pids");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let running = processes
            .local_record(first_process.id)
            .await?
            .is_some_and(|record| record.state == ProcessState::Running);
        let wrote_pid =
            std::fs::read_to_string(&pids_path).is_ok_and(|contents| contents.lines().count() == 1);
        if running && wrote_pid {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "generation 1 process did not reach running"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    shutdown.cancel();
    Ok(first_process.id)
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_before_recovery_terminates_running_unbound_process_after_restart() -> TestResult {
    use ma2a_core::service::{ProcessReason, ProcessState};

    use crate::processes::ProcessHosting;

    // Given: an Agent whose generation-1 supervisor received Go and started running, then
    // the daemon restarted before `bind_agent_process` committed.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-unbound-{}", u64::from_le_bytes(nonce)));
    std::fs::create_dir(&dir)?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let config = StoreConfig::new(&dir);
    let backend = StoreBackend::open(&config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let local = store.initialize().await?.endpoint_id;
    let agent = AgentId::new("restart-unbound")?;
    let first_id = launch_unbound_running_agent(&dir, (&config, &store, local), &agent).await?;

    let shutdown = CancellationToken::new();
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (&dir, Some(ProcessHosting::in_process())),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), Arc::new(SystemClock)),
        (&dir, processes.clone()),
        shutdown.clone(),
    );

    // When: operator stop acquires the Agent lock before startup recovery binds generation 1,
    // followed by a fresh start.
    manager.stop(&agent).await?;
    let after_stop = processes
        .local_record(first_id)
        .await?
        .ok_or("first process missing after stop")?;
    let second_pid = if after_stop.state.is_terminal() {
        manager.start(&agent).await?;
        manager
            .agent(&agent)
            .await?
            .and_then(|record| record.instance)
            .and_then(|instance| instance.process_id)
    } else {
        None
    };

    // Clean up any still-running processes so no child leaks on failure or success.
    let _ended_first = processes
        .terminate((None, first_id), (100, ProcessReason::Requested))
        .await;
    if let Some(pid) = second_pid {
        let _ended_second = processes
            .terminate((None, pid), (100, ProcessReason::Requested))
            .await;
    }
    let second_record = match second_pid {
        Some(pid) => processes.local_record(pid).await?,
        None => None,
    };
    shutdown.cancel();
    drop(manager);
    drop(processes);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    // Then: stop terminated the unbound generation-1 Process before returning, and the
    // replacement started only after generation 1 ended.
    assert_eq!(after_stop.state, ProcessState::Terminated);
    let first_ended = after_stop
        .times
        .ended_ms
        .ok_or("first process missing ended_ms")?;
    let second_record = second_record.ok_or("replacement process was not launched")?;
    assert!(
        second_record.times.created_ms >= first_ended,
        "replacement created ({}) before first ended ({first_ended})",
        second_record.times.created_ms
    );
    Ok(())
}
