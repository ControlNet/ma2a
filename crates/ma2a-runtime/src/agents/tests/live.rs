//! An Agent manager over a real Store holding one running task, stopped on demand.

use std::{path::PathBuf, sync::Arc, time::Duration};

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentMessage, AgentReply, AgentTaskId, AgentTaskReason, AgentTaskState, FrameKind,
        MAX_LOG_LIMIT, MessageRole, ProcessId, ProcessSpec, ProcessStart, ServiceError,
    },
};
use ma2a_services::{FrameReader, FrameWriter};
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

#[tokio::test]
async fn a_tool_call_cut_short_by_shutdown_stays_pending() -> TestResult {
    // Given: a pending call that waits for this Runtime's local API, which never binds.
    let live = Live::new().await?;
    let args = serde_json::json!({
        "endpoint": crate::agents::convert::hex(iroh::SecretKey::generate().public().as_bytes()),
        "agent": "other",
        "task": AgentTaskId::random()?.to_string(),
    });
    live.call(("c1", "agent.get", &args))?;
    live.manager.run_tool((live.task, "c1".to_owned()));
    tokio::time::sleep(Duration::from_millis(100)).await;

    // When: the daemon shuts down.
    live.shutdown.cancel();
    live.manager.0.tasks.close();
    let drained = tokio::time::timeout(Duration::from_secs(2), live.manager.0.tasks.wait()).await;

    // Then: the call ends at once with no answer, so a restart runs it again.
    let recorded = Repository::open(&live.config)?.tool_call((live.task, "c1"))?;
    live.finish().await?;
    drained.map_err(|_| "the call outlived the shutdown")?;
    let recorded = recorded.ok_or("the call was not recorded")?;
    assert_eq!(
        recorded.result, None,
        "the shutdown became the call's answer"
    );
    Ok(())
}

#[tokio::test]
async fn a_committed_cancellation_whose_wait_loses_the_store_is_of_unknown_outcome() -> TestResult {
    // Given: a running task whose cancellation is recorded and waits for its Agent.
    let live = Live::new().await?;
    let (manager, task) = (live.manager.clone(), live.task);
    let cancelling = tokio::spawn(async move {
        let stop = CancellationToken::new();
        let request = (task, AgentTaskReason::Requested, &stop);
        manager.cancel((None, None), request).await
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while Repository::open(&live.config)?
        .agent_task(task)?
        .is_some_and(|record| record.cancel.is_none())
    {
        assert!(tokio::time::Instant::now() < deadline, "never recorded");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // When: the store stops, and a change wakes the wait to read the task again.
    live.store.stop().await?;
    live.manager.changed();
    let cancelled = tokio::time::timeout(Duration::from_secs(5), cancelling).await??;

    // Then: the request stands, so its end is unknown rather than refused as if nothing
    // happened.
    drop(live.manager);
    live.store_task.await?;
    let _cleanup = std::fs::remove_dir_all(&live.dir);
    assert_eq!(cancelled.err(), Some(ServiceError::OutcomeUnknown));
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_artifact_blob_that_is_no_regular_file_is_refused_without_waiting() -> TestResult {
    // Given: a published artifact whose blob path holds a FIFO nobody writes to.
    let live = Live::new().await?;
    let artifact =
        ma2a_core::service::ArtifactRef::new(("report.txt", "text/plain"), (16, [5; 32]))?;
    Repository::open(&live.config)?.add_agent_artifact(live.task, (&artifact, 10))?;
    let blobs = live.dir.join("agents-v1").join("blobs");
    std::fs::create_dir_all(&blobs)?;
    let blob = blobs.join(crate::agents::convert::hex(&[5; 32]));
    let made = std::process::Command::new("mkfifo").arg(&blob).status()?;
    assert!(made.success(), "mkfifo failed");

    // When
    let read = live
        .manager
        .read_artifact((None, None), ((live.task, "report.txt"), 0));
    let read = tokio::time::timeout(Duration::from_secs(2), read).await;
    if read.is_err() {
        // Release the read still blocked on the FIFO, so the test can end.
        let _writer = std::fs::OpenOptions::new().write(true).open(&blob)?;
    }

    // Then: the blob is a broken store, answered at once.
    live.finish().await?;
    assert_eq!(
        read.map_err(|_| "the read waited on the FIFO")?.err(),
        Some(ServiceError::Internal)
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_artifact_blob_that_is_a_link_is_refused_even_to_content_of_its_size() -> TestResult {
    // Given: a published artifact whose blob path is a link to an unrelated file of its size.
    let live = Live::new().await?;
    let artifact =
        ma2a_core::service::ArtifactRef::new(("report.txt", "text/plain"), (16, [6; 32]))?;
    Repository::open(&live.config)?.add_agent_artifact(live.task, (&artifact, 10))?;
    let blobs = live.dir.join("agents-v1").join("blobs");
    std::fs::create_dir_all(&blobs)?;
    let unrelated = live.dir.join("unrelated");
    std::fs::write(&unrelated, b"unrelated secret")?;
    std::os::unix::fs::symlink(
        &unrelated,
        blobs.join(crate::agents::convert::hex(&[6; 32])),
    )?;

    // When
    let read = live
        .manager
        .read_artifact((None, None), ((live.task, "report.txt"), 0))
        .await;

    // Then: no byte of the linked file is answered as the artifact.
    live.finish().await?;
    assert_eq!(read.err(), Some(ServiceError::Internal));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_staging_file_that_vanishes_during_a_sweep_leaves_the_rest_swept() -> TestResult {
    // Given: old orphan blobs, and staging files that appear and vanish meanwhile, as
    // publications outside the sweep's lock make them.
    let live = Live::new().await?;
    let blobs = live.dir.join("agents-v1").join("blobs");
    std::fs::create_dir_all(&blobs)?;
    let old = std::time::SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
    let churning = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let churn = {
        let (blobs, churning) = (blobs.clone(), Arc::clone(&churning));
        std::thread::spawn(move || {
            let mut serial = 0_u64;
            while churning.load(std::sync::atomic::Ordering::Relaxed) {
                let staging = blobs.join(format!(".staging-{}", serial % 64));
                let _created = std::fs::write(&staging, b"x");
                let _removed = std::fs::remove_file(&staging);
                serial += 1;
            }
        })
    };

    // When: the sweep runs again and again.
    let mut kept = 0;
    for round in 0..200 {
        let orphans: Vec<_> = (0..16)
            .map(|index| blobs.join(format!("{round:04}{index:02}")))
            .collect();
        for orphan in &orphans {
            std::fs::File::create(orphan)?.set_modified(old)?;
        }
        live.manager.collect_blobs().await;
        kept += orphans.iter().filter(|orphan| orphan.exists()).count();
    }
    churning.store(false, std::sync::atomic::Ordering::Relaxed);
    churn.join().map_err(|_| "the churn thread panicked")?;

    // Then: every orphan went, whatever vanished beside it.
    live.finish().await?;
    assert_eq!(kept, 0);
    Ok(())
}

#[tokio::test]
async fn a_watch_hears_its_cancellation_while_history_is_still_sent() -> TestResult {
    // Given: an ended task with far more retained events than one batch read.
    let live = Live::new().await?;
    let mut repository = Repository::open(&live.config)?;
    let output = |cursor| AgentOutput {
        agent: live.agent.clone(),
        generation: live.generation,
        cursor,
        at_ms: 3_000,
    };
    for cursor in 100..300 {
        let note = AgentMessage::text(MessageRole::Agent, "note")?;
        let effect = AgentOutputEffect::Message(live.task, note);
        repository.apply_agent_output(&output(cursor), effect)?;
    }
    let ended = AgentOutputEffect::State {
        task: live.task,
        state: AgentTaskState::Completed,
        message: None,
        error: None,
    };
    repository.apply_agent_output(&output(300), ended)?;

    // When: the caller cancels the watch once it is accepted, and reads what follows.
    let (caller, host) = tokio::io::duplex(1_024);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (mut caller_reader, mut caller_writer) = (
        FrameReader::new(caller_read),
        FrameWriter::new(caller_write),
    );
    let (host_read, host_write) = tokio::io::split(host);
    let (mut host_reader, mut host_writer) =
        (FrameReader::new(host_read), FrameWriter::new(host_write));
    let stop = CancellationToken::new();
    let watched = live.manager.watch(
        ((None, None), &stop, (live.task, 0)),
        (&mut host_reader, &mut host_writer),
    );
    let caller = async {
        let accept = caller_reader.next().await?.ok_or("no accept")?;
        assert_eq!(accept.kind(), FrameKind::Accept);
        caller_writer.frame(FrameKind::Cancel, &[]).await?;
        caller_writer.flush().await?;
        let mut events = 0;
        loop {
            let frame = caller_reader.next().await?.ok_or("no outcome")?;
            if frame.kind() != FrameKind::Data {
                let outcome = AgentReply::decode_outcome(frame.payload())?;
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>((events, outcome));
            }
            events += 1;
        }
    };
    let (_report, read) = tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(watched, caller)
    })
    .await?;
    let (events, outcome) = read?;

    // Then: the watch ends cancelled, without sending the rest of the history first.
    live.finish().await?;
    assert_eq!(outcome, Err(ServiceError::Cancelled));
    assert!(
        events < 200,
        "{events} events were sent after the cancellation"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_that_fails_its_task_at_the_event_bound_is_a_committed_send() -> TestResult {
    // Given: a running task with room for no more durable events.
    let live = Live::new().await?;
    let mut repository = Repository::open(&live.config)?;
    for cursor in 0..u64::from(ma2a_store::MAX_DURABLE_EVENTS) - 2 {
        let output = AgentOutput {
            agent: live.agent.clone(),
            generation: live.generation,
            cursor: 10 + cursor,
            at_ms: 3_000,
        };
        let note = AgentMessage::text(MessageRole::Agent, "note")?;
        repository.apply_agent_output(&output, AgentOutputEffect::Message(live.task, note))?;
    }
    let before = repository.revision()?;

    // When: the operator sends one more message.
    let message = AgentMessage::text(MessageRole::User, "more")?;
    let sent = live.manager.send((None, None), (live.task, message)).await;

    // Then: the send is answered as having failed the task at the revision it committed, not
    // as a send to a task that had already ended.
    let after = repository.revision()?;
    let state = repository.agent_task(live.task)?.ok_or("missing")?.summary;
    live.finish().await?;
    assert!(matches!(sent, Ok(super::Sent::FailedTask(revision)) if revision == after));
    assert_eq!(after, before + 1);
    assert_eq!(state.reason, Some(AgentTaskReason::Limit));
    Ok(())
}
