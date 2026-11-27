//! Maintenance keeps task cancellation expiry moving while slow Agent startups run separately.
#![cfg(unix)]

use std::{path::PathBuf, sync::Arc, time::Duration};

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentId, AgentMessage, AgentTaskId, AgentTaskReason, AgentTaskState, MAX_LOG_LIMIT,
        MessageRole, ProcessId, ProcessSpec, ProcessStart, ProcessState,
    },
};
use ma2a_store::{
    AgentDefinition, AgentOutput, AgentOutputEffect, AgentTaskCreation, NewAgentTask, NewProcess,
    ProcessTransition, Repository, StoreConfig, TASK_ENDED_ANSWER,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::AgentManager;
use crate::{
    RuntimeClock,
    processes::{ProcessHosting, ProcessManager},
    store::{STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Clock tied to Tokio's paused time so `tokio::time::advance` drives `now_ms()`.
#[derive(Debug)]
struct TokioClock {
    epoch: tokio::time::Instant,
    base_ms: i64,
}

impl TokioClock {
    fn new(base_ms: i64) -> Self {
        Self {
            epoch: tokio::time::Instant::now(),
            base_ms,
        }
    }
}

impl RuntimeClock for TokioClock {
    fn now_ms(&self) -> Result<i64, crate::RuntimeError> {
        let elapsed = i64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(i64::MAX);
        Ok(self.base_ms.saturating_add(elapsed))
    }
}

fn seed_agents(config: &StoreConfig, local: EndpointId) -> TestResult<AgentTaskId> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(config)?;
    for name in ["alpha-1", "alpha-2"] {
        let id = AgentId::new(name)?;
        repository.define_agent(
            &AgentDefinition {
                id: id.clone(),
                description: String::new(),
                start: start_bytes.clone(),
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?;
        repository.set_agent_enabled(&id, (true, 2))?;
    }

    let beta = AgentId::new("beta")?;
    repository.define_agent(
        &AgentDefinition {
            id: beta.clone(),
            description: String::new(),
            start: start_bytes.clone(),
            tools: ma2a_agent::tools::ToolFamily::Agent.bit(),
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&beta, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&beta, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no beta instance")?
        .generation;
    let beta_pid = ProcessId::from_bytes([0x55; 16]);
    repository.create_process(&NewProcess {
        id: beta_pid,
        binding: (local, RequestId::random()?),
        fingerprint: [0x66; 32],
        start: (&start_bytes, None),
        secret: [0x77; 32],
        created_ms: 4,
    })?;
    for (to, at_ms) in [(ProcessState::Starting, 5), (ProcessState::Running, 6)] {
        repository.transition_process(
            beta_pid,
            ProcessTransition {
                to,
                reason: None,
                termination: None,
                at_ms,
            },
        )?;
    }
    repository.bind_agent_process((&beta, generation), beta_pid)?;
    repository.apply_agent_output(
        &AgentOutput {
            agent: beta.clone(),
            generation,
            cursor: 1,
            at_ms: 7,
        },
        AgentOutputEffect::Ready,
    )?;
    let task_id = AgentTaskId::random()?;
    let AgentTaskCreation::Created(task) = repository
        .create_agent_task(&NewAgentTask {
            id: task_id,
            agent: beta.clone(),
            binding: (local, RequestId::random()?),
            fingerprint: [0x88; 32],
            from_agent: None,
            input: AgentMessage::text(MessageRole::User, "work")?,
            created_ms: 8,
        })?
        .into_value()
    else {
        return Err("task was not created".into());
    };
    repository.assign_agent_tasks(&beta, 9)?;
    repository.apply_agent_output(
        &AgentOutput {
            agent: beta,
            generation,
            cursor: 2,
            at_ms: 10,
        },
        AgentOutputEffect::ToolCall {
            task: task.summary.id,
            call: "c1".to_owned(),
            name: "agent.get".to_owned(),
            request: b"{}".to_vec(),
            rejection: b"{\"error\":{\"code\":\"concurrency_exceeded\"}}".to_vec(),
        },
    )?;
    Ok(task.summary.id)
}

#[tokio::test(start_paused = true)]
async fn slow_agent_startups_do_not_delay_cancellation_expiry() -> TestResult {
    // Given: two enabled Agents whose supervisor binary never binds (each holding startup for 10 s),
    // and a third ready Agent with a running task and an open tool call.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-maintain-{}", u64::from_le_bytes(nonce)));
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

    let clock: Arc<dyn RuntimeClock> = Arc::new(TokioClock::new(1_000_000));
    let shutdown = CancellationToken::new();
    let hosting = ProcessHosting::executable(
        PathBuf::from("/bin/sh"),
        vec!["-c".into(), "cat >/dev/null".into()],
    );
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (&dir, Some(hosting)),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), clock),
        (&dir, processes),
        shutdown.clone(),
    );

    let beta_task = seed_agents(&config, local)?;
    // Mark the tool call as currently running so reconcile's pending-call sweep leaves it open
    // until cancellation grace expires.
    manager
        .coordination()
        .calls
        .insert((beta_task, "c1".to_owned()));

    // When: cancellation is requested at t=0 and the maintenance loop runs for 12 seconds.
    manager
        .request_cancel((None, None), (beta_task, AgentTaskReason::Requested))
        .await?;
    let runner = tokio::spawn(crate::agents::run(manager.clone()));
    for _ in 0..8 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
    }
    for _ in 0..12 {
        for _ in 0..50 {
            tokio::time::advance(Duration::from_millis(20)).await;
            tokio::task::yield_now().await;
        }
        // Flush the multi-step Store round trips of one maintenance tick.
        for _ in 0..8 {
            let _rev = store.revision().await?;
            tokio::task::yield_now().await;
        }
    }

    // Then: beta's task expired within the 10 s grace and its open tool call got `task_ended`,
    // while alpha-1 and alpha-2 ran their slow startup attempts independently.
    let task_record = Repository::open(&config)?.agent_task(beta_task)?;
    let tool_call = Repository::open(&config)?.tool_call((beta_task, "c1"))?;
    let alpha_1 = Repository::open(&config)?.agent(&AgentId::new("alpha-1")?)?;
    let alpha_2 = Repository::open(&config)?.agent(&AgentId::new("alpha-2")?)?;

    shutdown.cancel();
    for _ in 0..20 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
        if runner.is_finished() {
            break;
        }
    }
    let clean_shutdown = runner.is_finished();
    if !clean_shutdown {
        runner.abort();
    }
    let _joined = runner.await;
    drop(manager);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    assert!(clean_shutdown, "maintenance should shut down promptly");
    for alpha in [alpha_1, alpha_2] {
        let record = alpha.ok_or("slow agent missing")?;
        assert!(
            record.generation >= 1,
            "slow agent never attempted startup: {record:?}"
        );
    }
    let task_record = task_record.ok_or("task missing")?;
    assert_eq!(
        task_record.summary.state,
        AgentTaskState::Cancelled,
        "task cancellation should expire within grace despite slow Agent startups"
    );
    assert_eq!(task_record.summary.reason, Some(AgentTaskReason::Requested));
    let tool_call = tool_call.ok_or("tool call missing")?;
    assert_eq!(
        tool_call.result.as_deref(),
        Some(TASK_ENDED_ANSWER),
        "open tool call should be answered task_ended on cancellation expiry"
    );
    Ok(())
}

type FailingAgent = (AgentId, ProcessId, [u8; 32]);

fn seed_failing_predecessor_agents(
    config: &StoreConfig,
    local: EndpointId,
    state: ProcessState,
) -> TestResult<Vec<FailingAgent>> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(config)?;
    let mut failing = Vec::with_capacity(8);
    for index in 1_u8..=8 {
        let id = AgentId::new(&format!("fail-{index}"))?;
        repository.define_agent(
            &AgentDefinition {
                id: id.clone(),
                description: String::new(),
                start: start_bytes.clone(),
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?;
        repository.set_agent_enabled(&id, (true, 2))?;
        let generation = repository
            .begin_agent_instance(&id, 3)?
            .into_value()
            .and_then(|record| record.instance)
            .ok_or("no failing instance")?
            .generation;
        let pid = ProcessId::from_bytes([0x90 + index; 16]);
        let secret = [0xA0 + index; 32];
        repository.create_process(&NewProcess {
            id: pid,
            binding: (local, RequestId::random()?),
            fingerprint: [0xB0 + index; 32],
            start: (&start_bytes, None),
            secret,
            created_ms: 4,
        })?;
        if state == ProcessState::Running {
            for (to, at_ms) in [(ProcessState::Starting, 5), (ProcessState::Running, 6)] {
                repository.transition_process(
                    pid,
                    ProcessTransition {
                        to,
                        reason: None,
                        termination: None,
                        at_ms,
                    },
                )?;
            }
        }
        repository.bind_agent_process((&id, generation), pid)?;
        repository.set_agent_enabled(&id, (false, 7))?;
        repository.set_agent_enabled(&id, (true, 8))?;
        failing.push((id, pid, secret));
    }
    let healthy = AgentId::new("healthy")?;
    repository.define_agent(
        &AgentDefinition {
            id: healthy.clone(),
            description: String::new(),
            start: start_bytes,
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&healthy, (true, 2))?;
    Ok(failing)
}

#[tokio::test(start_paused = true)]
async fn eight_failing_predecessor_recoveries_do_not_starve_ninth_healthy_agent() -> TestResult {
    // Given: eight enabled Agents (`fail-1`..`fail-8`) whose previous Processes cannot be
    // confirmed terminal, ahead in name order of a ninth healthy Agent (`healthy`).
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-starve-{}", u64::from_le_bytes(nonce)));
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
    let failing = seed_failing_predecessor_agents(&config, local, ProcessState::Created)?;

    let clock: Arc<dyn RuntimeClock> = Arc::new(TokioClock::new(1_000_000));
    let shutdown = CancellationToken::new();
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (&dir, None),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), clock),
        (&dir, processes),
        shutdown.clone(),
    );

    // When: maintenance runs for 5 ticks.
    let runner = tokio::spawn(crate::agents::run(manager.clone()));
    for _ in 0..8 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
    }
    for _ in 0..5 {
        for _ in 0..50 {
            tokio::time::advance(Duration::from_millis(20)).await;
            tokio::task::yield_now().await;
        }
        for _ in 0..8 {
            let _rev = store.revision().await?;
            tokio::task::yield_now().await;
        }
    }

    let healthy = Repository::open(&config)?.agent(&AgentId::new("healthy")?)?;
    let backoffs: Vec<_> = {
        let coordination = manager.coordination();
        failing
            .iter()
            .map(|(id, _, _)| {
                (
                    id.clone(),
                    coordination.restarts.get(id).map(|(_, next)| *next),
                )
            })
            .collect()
    };

    shutdown.cancel();
    for _ in 0..20 {
        let _rev = store.revision().await?;
        tokio::task::yield_now().await;
        if runner.is_finished() {
            break;
        }
    }
    if !runner.is_finished() {
        runner.abort();
    }
    let _joined = runner.await;
    drop(manager);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    // Then: `healthy` claimed a start slot and began its first instance, while each failing
    // predecessor recovery was backed off rather than hammering every tick.
    let healthy = healthy.ok_or("healthy agent missing")?;
    assert!(
        healthy.instance.is_some(),
        "healthy agent starved behind eight failing predecessor recoveries: {healthy:?}"
    );
    for (id, next_backoff) in backoffs {
        assert!(
            next_backoff.is_some_and(|next| next >= Duration::from_secs(2)),
            "failing predecessor recovery for {id} did not schedule retry backoff: {next_backoff:?}"
        );
    }
    Ok(())
}

async fn run_dropping_supervisor(
    (address, credentials): (
        ma2a_services::supervisor::SupervisorAddress,
        ma2a_services::supervisor::Credentials,
    ),
    (connected, mut release, shutdown): (
        Arc<std::sync::atomic::AtomicUsize>,
        tokio::sync::watch::Receiver<bool>,
        CancellationToken,
    ),
) -> TestResult {
    use interprocess::local_socket::tokio::prelude::*;
    use ma2a_services::{
        FrameReader, FrameWriter,
        supervisor::proof::{prove_supervisor, read_challenge},
    };

    let listener = address.bind()?;
    loop {
        let mut stream = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            accepted = listener.accept() => accepted?,
        };
        let challenge = read_challenge(&mut FrameReader::new(&mut stream)).await?;
        let (recv, send) = stream.split();
        let (mut reader, mut writer) = (FrameReader::new(recv), FrameWriter::new(send));
        prove_supervisor(challenge, (&mut reader, &mut writer), &credentials).await?;
        let _request = reader.expect().await?;
        connected.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let _released = tokio::select! {
            () = shutdown.cancelled() => return Ok(()),
            res = release.wait_for(|open| *open) => res,
        };
        drop((reader, writer));
    }
}

fn spawn_dropping_supervisors(
    (sockets_dir, failing): (&std::path::Path, &[FailingAgent]),
    (connected, release, shutdown): (
        &Arc<std::sync::atomic::AtomicUsize>,
        &tokio::sync::watch::Receiver<bool>,
        &CancellationToken,
    ),
) -> Vec<tokio::task::JoinHandle<TestResult>> {
    use ma2a_services::supervisor::{Credentials, SupervisorAddress};

    failing
        .iter()
        .map(|(_, pid, secret)| {
            let address = SupervisorAddress::new(
                sockets_dir
                    .join(format!("{pid}.sock"))
                    .to_string_lossy()
                    .into_owned(),
            );
            tokio::spawn(run_dropping_supervisor(
                (address, Credentials::new(*pid, *secret)),
                (Arc::clone(connected), release.clone(), shutdown.clone()),
            ))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ninth_healthy_agent_claims_slot_when_failing_supervisor_releases_permit() -> TestResult {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Given: eight enabled Agents (`fail-1`..`fail-8`) whose running predecessors have mock
    // supervisors that accept Terminate and drop without answering, plus a ninth healthy Agent.
    let mut nonce = [0_u8; 8];
    getrandom::fill(&mut nonce)?;
    let dir = std::env::temp_dir().join(format!("ma2a-slot-{}", u64::from_le_bytes(nonce)));
    std::fs::create_dir(&dir)?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let sockets_dir = dir.join("run-v1").join("p");
    std::fs::create_dir_all(&sockets_dir)?;
    let config = StoreConfig::new(&dir);
    let backend = StoreBackend::open(&config)?;
    let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
    let store_task = tokio::task::spawn_blocking(move || backend.run(receiver));
    let store = StoreClient::new(sender);
    let local = store.initialize().await?.endpoint_id;
    let failing = seed_failing_predecessor_agents(&config, local, ProcessState::Running)?;

    let shutdown = CancellationToken::new();
    let (release_tx, release_rx) = tokio::sync::watch::channel(false);
    let connected = Arc::new(AtomicUsize::new(0));
    let servers = spawn_dropping_supervisors(
        (&sockets_dir, &failing),
        (&connected, &release_rx, &shutdown),
    );

    let clock: Arc<dyn RuntimeClock> = Arc::new(crate::clock::SystemClock);
    let processes = ProcessManager::new(
        (local, store.clone(), Arc::clone(&clock)),
        (&dir, None),
        shutdown.clone(),
    );
    let manager = AgentManager::new(
        (local, store.clone(), clock),
        (&dir, processes),
        shutdown.clone(),
    );

    // When: maintenance fills all 8 start permits with the 8 failing predecessor recoveries,
    // then the supervisors drop their Terminate connections to free the permits.
    let runner = tokio::spawn(crate::agents::run(manager.clone()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while connected.load(Ordering::SeqCst) < 8 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "not all 8 failing recoveries reached their mock supervisors"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(manager.0.start_permits.available_permits(), 0);
    let healthy_id = AgentId::new("healthy")?;
    let before_release = Repository::open(&config)?
        .agent(&healthy_id)?
        .ok_or("healthy missing")?;
    assert_eq!(before_release.instance, None);

    let _sent = release_tx.send(true);
    let healthy_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut healthy_started = false;
    while tokio::time::Instant::now() < healthy_deadline {
        if Repository::open(&config)?
            .agent(&healthy_id)?
            .and_then(|record| record.instance)
            .is_some()
        {
            healthy_started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let backoffs: Vec<_> = {
        let coordination = manager.coordination();
        failing
            .iter()
            .map(|(id, _, _)| (id.clone(), coordination.restarts.contains_key(id)))
            .collect()
    };

    shutdown.cancel();
    let _joined = tokio::time::timeout(Duration::from_secs(5), runner).await;
    for server in servers {
        let _joined = tokio::time::timeout(Duration::from_secs(2), server).await;
    }
    drop(manager);
    store.stop().await?;
    store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&dir);

    // Then: `healthy` claimed a slot as soon as one freed up, and failing predecessors backed off.
    assert!(
        healthy_started,
        "healthy agent never claimed a start slot after failing supervisors dropped Terminate"
    );
    for (id, backed_off) in backoffs {
        assert!(
            backed_off,
            "failing predecessor recovery for {id} did not record retry backoff"
        );
    }
    Ok(())
}
