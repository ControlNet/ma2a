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

/// Watches `live`'s task from the start until the watch ends, the caller having sent `data`
/// on it; the kinds of the frames the caller read after `accept`, and the outcome among them.
async fn watched(
    live: &Live,
    stop: &CancellationToken,
    data: Option<&[u8]>,
) -> TestResult<(Vec<FrameKind>, Option<Result<AgentReply, ServiceError>>)> {
    let (caller, host) = tokio::io::duplex(1 << 16);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (mut caller_reader, mut caller_writer) = (
        FrameReader::new(caller_read),
        FrameWriter::new(caller_write),
    );
    let (host_read, host_write) = tokio::io::split(host);
    let (mut host_reader, mut host_writer) =
        (FrameReader::new(host_read), FrameWriter::new(host_write));
    if let Some(data) = data {
        caller_writer
            .data(ma2a_core::service::Channel::Input, data)
            .await?;
        caller_writer.flush().await?;
    }
    let watch = live.manager.watch(
        ((None, None), stop, (live.task, 0)),
        (&mut host_reader, &mut host_writer),
    );
    tokio::time::timeout(Duration::from_secs(10), watch).await?;
    drop((host_reader, host_writer));
    let accept = caller_reader.next().await?.ok_or("no accept")?;
    assert_eq!(accept.kind(), FrameKind::Accept);
    let (mut kinds, mut outcome) = (Vec::new(), None);
    while let Some(frame) = caller_reader.next().await? {
        kinds.push(frame.kind());
        if frame.kind() == FrameKind::Outcome {
            outcome = Some(AgentReply::decode_outcome(frame.payload())?);
        }
    }
    Ok((kinds, outcome))
}

#[tokio::test]
async fn a_watch_the_runtime_stops_ends_unavailable() -> TestResult {
    // Given: a running task with an event to send, and a Runtime that is stopping.
    let live = Live::new().await?;
    let stop = CancellationToken::new();
    stop.cancel();

    // When
    let (kinds, outcome) = watched(&live, &stop, None).await?;

    // Then: the caller is told the watch ended unavailable, not left with a broken stream.
    live.finish().await?;
    assert_eq!(kinds.last(), Some(&FrameKind::Outcome));
    assert_eq!(outcome, Some(Err(ServiceError::Unavailable)));
    Ok(())
}

#[tokio::test]
async fn a_watch_given_anything_but_cancel_ends_invalid_input() -> TestResult {
    // Given: a running task, whose watcher sends data on its watch.
    let live = Live::new().await?;
    let stop = CancellationToken::new();

    // When
    let (_, outcome) = watched(&live, &stop, Some(b"more")).await?;

    // Then
    live.finish().await?;
    assert_eq!(outcome, Some(Err(ServiceError::InvalidInput)));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_answer_the_store_refused_once_is_recorded_when_it_recovers() -> TestResult {
    // Given: a pending call of a tool that does not exist, answered at once without any
    // effect, while the Store refuses every change to a tool call.
    let live = Live::new().await?;
    live.call(("c1", "no.such.tool", &serde_json::json!({})))?;
    let sql = rusqlite::Connection::open(live.config.database_path())?;
    sql.execute_batch(
        "CREATE TRIGGER test_tool_answer_failure BEFORE UPDATE ON agent_tool_calls
         BEGIN SELECT RAISE(ABORT, 'injected tool answer failure'); END;",
    )?;

    // When: the call runs, and the Store recovers after the answer was refused.
    live.manager.run_tool((live.task, "c1".to_owned()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    sql.execute_batch("DROP TRIGGER test_tool_answer_failure;")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let recorded = loop {
        let call = Repository::open(&live.config)?.tool_call((live.task, "c1"))?;
        if let Some(result) = call.and_then(|call| call.result) {
            break Some(result);
        }
        if tokio::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Then: the answer the call already computed is recorded, without a restart.
    live.finish().await?;
    let recorded = recorded.ok_or("the answer was never recorded")?;
    assert!(String::from_utf8(recorded)?.contains("invalid_tool"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_ready_instance_is_not_overdue_while_the_store_fails_for_a_moment() -> TestResult {
    // Given: a ready instance, and a Store whose reads of instances fail for a moment.
    let live = Live::new().await?;
    let sql = rusqlite::Connection::open(live.config.database_path())?;
    sql.execute_batch("ALTER TABLE agent_instances RENAME TO agent_instances_away;")?;
    let restoring = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        sql.execute_batch("ALTER TABLE agent_instances_away RENAME TO agent_instances;")
    });

    // When: its ready deadline asks whether it is ready.
    let overdue = tokio::time::timeout(
        Duration::from_secs(10),
        live.manager.overdue(&live.agent, live.generation),
    )
    .await;

    // Then: a failed read is no reason to end it.
    restoring.await??;
    live.finish().await?;
    assert_eq!(overdue.ok(), Some(false));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_instance_is_still_followed_while_the_store_fails_for_a_moment() -> TestResult {
    // Given: a live instance, and a Store whose reads of instances fail for a moment.
    let live = Live::new().await?;
    let sql = rusqlite::Connection::open(live.config.database_path())?;
    sql.execute_batch("ALTER TABLE agent_instances RENAME TO agent_instances_away;")?;
    let restoring = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        sql.execute_batch("ALTER TABLE agent_instances_away RENAME TO agent_instances;")
    });

    // When: its link looks the instance up before reaching its supervisor.
    let found = tokio::time::timeout(
        Duration::from_secs(10),
        live.manager.live_instance(&live.agent, live.generation),
    )
    .await;

    // Then: a failed read is no reason to stop following it.
    restoring.await??;
    let generation = live.generation;
    live.finish().await?;
    assert_eq!(
        found.ok().flatten().map(|instance| instance.generation),
        Some(generation)
    );
    Ok(())
}

#[tokio::test]
async fn a_task_and_its_artifacts_are_read_as_of_one_moment() -> TestResult {
    // Given: a running task, its newest event, and a store thread held until it is released.
    let live = Live::new().await?;
    let before = Repository::open(&live.config)?
        .agent_task(live.task)?
        .ok_or("no task")?
        .summary
        .last_seq;
    let (release, held) = std::sync::mpsc::channel::<()>();
    let store = live.store.clone();
    let holding = tokio::spawn(async move { store.processes(move |_| held.recv()).await });
    tokio::task::yield_now().await;

    // When: the task is read, and an artifact published between the reads the store queued.
    let (manager, task) = (live.manager.clone(), live.task);
    let reading = tokio::spawn(async move { manager.details((None, None), task).await });
    tokio::task::yield_now().await;
    let artifact =
        ma2a_core::service::ArtifactRef::new(("report.txt", "text/plain"), (16, [5; 32]))?;
    let store = live.store.clone();
    let publishing = tokio::spawn(async move {
        store
            .processes(move |repository| repository.add_agent_artifact(task, (&artifact, 10)))
            .await
    });
    tokio::task::yield_now().await;
    release.send(())?;
    let _held = holding.await?;
    publishing.await???;
    let details = reading.await??;

    // Then: the artifact is listed exactly when the task's newest event counts it.
    live.finish().await?;
    let seen = (details.artifacts.len(), details.summary.last_seq);
    assert!(
        seen == (0, before) || seen == (1, before + 1),
        "{seen:?} after {before}"
    );
    Ok(())
}

/// Records the bound Process `[4; 16]` as exited and writes `records` to its durable log.
fn write_ended_instance_log(
    live: &Live,
    records: &[(ma2a_core::service::Channel, &[u8])],
) -> TestResult {
    use ma2a_core::service::{ProcessState, Termination};
    use ma2a_services::supervisor::{log::LogWriter, log_directory};
    use ma2a_store::{NewProcess, ProcessTransition};

    let process = ProcessId::from_bytes([4; 16]);
    let mut repository = Repository::open(&live.config)?;
    let local = repository.endpoint()?.ok_or("no endpoint")?.endpoint_id();
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let new = NewProcess {
        id: process,
        binding: (local, RequestId::random()?),
        fingerprint: [7; 32],
        start: (&start, None),
        secret: [8; 32],
        created_ms: 3,
    };
    repository.create_process(&new)?;
    repository.transition_process(
        process,
        ProcessTransition {
            to: ProcessState::Starting,
            reason: None,
            termination: None,
            at_ms: 4,
        },
    )?;
    repository.transition_process(
        process,
        ProcessTransition {
            to: ProcessState::Running,
            reason: None,
            termination: None,
            at_ms: 5,
        },
    )?;
    repository.transition_process(
        process,
        ProcessTransition {
            to: ProcessState::Exited,
            reason: None,
            termination: Some(Termination::Exited(0)),
            at_ms: 9,
        },
    )?;
    let dir = live.dir.join("processes-v1").join(process.to_string());
    let mut log = LogWriter::create(&log_directory(&dir), u64::from(MAX_LOG_LIMIT))?;
    for (channel, bytes) in records {
        log.append(*channel, bytes)?;
    }
    log.sync()?;
    Ok(())
}

#[tokio::test]
async fn an_ended_instance_drains_unread_stdout_before_concluding() -> TestResult {
    // Given: a live instance at cursor 1 whose Process already exited after writing a
    // stderr line and a final `completed` line with an inline result, plus a queued task.
    let live = Live::new().await?;
    let mut repository = Repository::open(&live.config)?;
    let local = repository.endpoint()?.ok_or("no endpoint")?.endpoint_id();
    let queued_task = NewAgentTask {
        id: AgentTaskId::random()?,
        agent: live.agent.clone(),
        binding: (local, RequestId::random()?),
        fingerprint: [9; 32],
        from_agent: None,
        input: AgentMessage::text(MessageRole::User, "next")?,
        created_ms: 8,
    };
    let AgentTaskCreation::Created(queued) =
        repository.create_agent_task(&queued_task)?.into_value()
    else {
        return Err("queued task was not created".into());
    };
    drop(repository);

    let completed = serde_json::json!({
        "t": "state",
        "task": live.task.to_string(),
        "state": "completed",
        "message": {"role": "agent", "parts": [{"kind": "text", "text": "drained result"}]},
    });
    let mut payload = serde_json::to_vec(&completed)?;
    payload.push(b'\n');
    write_ended_instance_log(
        &live,
        &[
            (ma2a_core::service::Channel::Output, b"\n"),
            (ma2a_core::service::Channel::Error, b"stderr note\n"),
            (ma2a_core::service::Channel::Output, &payload),
        ],
    )?;

    // When: the link follows the ended instance.
    tokio::time::timeout(
        Duration::from_secs(5),
        live.manager.follow_instance(&live.agent, live.generation),
    )
    .await?;

    // Then: the unread completion applied, the queued task stayed queued for the next
    // instance, and the ended instance is concluded.
    let repository = Repository::open(&live.config)?;
    let record = repository.agent_task(live.task)?.ok_or("missing task")?;
    let queued_record = repository
        .agent_task(queued.summary.id)?
        .ok_or("missing queued")?;
    let instance = repository
        .agent(&live.agent)?
        .and_then(|agent| agent.instance)
        .ok_or("missing instance")?;
    live.finish().await?;
    assert_eq!(record.summary.state, AgentTaskState::Completed);
    assert_eq!(
        record.result,
        Some(AgentMessage::text(MessageRole::Agent, "drained result")?)
    );
    assert_eq!(queued_record.summary.state, AgentTaskState::Queued);
    assert!(!instance.is_live());
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_transient_store_failure_during_ended_drain_retries_before_concluding() -> TestResult {
    // Given: an ended instance with an unread `completed` line while the Store briefly rejects
    // task event writes.
    let live = Live::new().await?;
    let completed = serde_json::json!({
        "t": "state",
        "task": live.task.to_string(),
        "state": "completed",
        "message": {"role": "agent", "parts": [{"kind": "text", "text": "retried result"}]},
    });
    let mut payload = serde_json::to_vec(&completed)?;
    payload.push(b'\n');
    write_ended_instance_log(
        &live,
        &[
            (ma2a_core::service::Channel::Output, b"\n"),
            (ma2a_core::service::Channel::Output, &payload),
        ],
    )?;
    let sql = rusqlite::Connection::open(live.config.database_path())?;
    sql.execute_batch("ALTER TABLE agent_task_events RENAME TO agent_task_events_away;")?;
    let restoring = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        sql.execute_batch("ALTER TABLE agent_task_events_away RENAME TO agent_task_events;")
    });

    // When: the link follows the instance through the transient failure.
    tokio::time::timeout(
        Duration::from_secs(10),
        live.manager.follow_instance(&live.agent, live.generation),
    )
    .await?;
    restoring.await??;

    // Then: the link retried instead of concluding early, and the task completed.
    let record = Repository::open(&live.config)?
        .agent_task(live.task)?
        .ok_or("missing task")?;
    live.finish().await?;
    assert_eq!(record.summary.state, AgentTaskState::Completed);
    assert_eq!(
        record.result,
        Some(AgentMessage::text(MessageRole::Agent, "retried result")?)
    );
    Ok(())
}

#[tokio::test]
async fn a_watch_stopped_mid_event_frame_finishes_the_frame_and_sends_unavailable() -> TestResult {
    use tokio::io::AsyncReadExt as _;

    // Given: a running task with enough queued events to block `writer.data` over a small pipe.
    let live = Live::new().await?;
    let mut repository = Repository::open(&live.config)?;
    let body = "x".repeat(512);
    for cursor in 100..116 {
        let note = AgentMessage::text(MessageRole::Agent, &body)?;
        repository.apply_agent_output(
            &AgentOutput {
                agent: live.agent.clone(),
                generation: live.generation,
                cursor,
                at_ms: 3_000,
            },
            AgentOutputEffect::Message(live.task, note),
        )?;
    }

    let (caller, host) = tokio::io::duplex(256);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (mut caller_reader, _caller_writer) = (
        FrameReader::new(caller_read),
        FrameWriter::new(caller_write),
    );
    let (host_read, host_write) = tokio::io::split(host);
    let (mut host_reader, mut host_writer) =
        (FrameReader::new(host_read), FrameWriter::new(host_write));
    let stop = CancellationToken::new();
    let watched = async {
        let report = live
            .manager
            .watch(
                ((None, None), &stop, (live.task, 0)),
                (&mut host_reader, &mut host_writer),
            )
            .await;
        drop((host_reader, host_writer));
        report
    };

    // When: the caller reads `accept` and the start of the first `data` frame, then Runtime
    // stops while the rest of that frame is still blocked, and the caller drains the stream.
    let caller = async {
        let accept = caller_reader.next().await?.ok_or("no accept")?;
        assert_eq!(accept.kind(), FrameKind::Accept);
        let mut caller_read = caller_reader.into_inner();
        let mut head = [0_u8; 8];
        caller_read.read_exact(&mut head).await?;
        stop.cancel();
        let mut bytes = head.to_vec();
        caller_read.read_to_end(&mut bytes).await?;
        let mut frames = FrameReader::new(bytes.as_slice());
        let first = frames.next().await?.ok_or("missing first data frame")?;
        assert_eq!(first.kind(), FrameKind::Data);
        let mut outcome = None;
        while let Some(frame) = frames.next().await? {
            if frame.kind() == FrameKind::Outcome {
                outcome = Some(AgentReply::decode_outcome(frame.payload())?);
            }
        }
        TestResult::Ok(outcome)
    };
    let (report, outcome) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(watched, caller)
    })
    .await?;

    // Then: the started frame finished cleanly and `Outcome(Unavailable)` followed.
    live.finish().await?;
    assert_eq!(
        report.end,
        ma2a_services::OperationEnd::Failed(ServiceError::Unavailable)
    );
    assert_eq!(outcome?, Some(Err(ServiceError::Unavailable)));
    Ok(())
}

#[tokio::test]
async fn a_watch_hears_caller_cancel_and_eof_while_event_write_is_blocked() -> TestResult {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    for send_cancel in [true, false] {
        let live = Live::new().await?;
        let after = {
            let mut repository = Repository::open(&live.config)?;
            let initial = repository.agent_task_events(live.task, (0, 64))?;
            let after = initial.last().map_or(0, |event| event.seq);
            let body = "y".repeat(512);
            for cursor in 100..116 {
                let note = AgentMessage::text(MessageRole::Agent, &body)?;
                repository.apply_agent_output(
                    &AgentOutput {
                        agent: live.agent.clone(),
                        generation: live.generation,
                        cursor,
                        at_ms: 3_000,
                    },
                    AgentOutputEffect::Message(live.task, note),
                )?;
            }
            after
        };

        let (caller, host) = tokio::io::duplex(256);
        let (caller_read, caller_write) = tokio::io::split(caller);
        let (mut caller_reader, mut caller_writer) = (
            FrameReader::new(caller_read),
            FrameWriter::new(caller_write),
        );
        let (host_read, host_write) = tokio::io::split(host);
        let (mut host_reader, mut host_writer) =
            (FrameReader::new(host_read), FrameWriter::new(host_write));
        let stop = CancellationToken::new();
        let watched = async {
            let report = live
                .manager
                .watch(
                    ((None, None), &stop, (live.task, after)),
                    (&mut host_reader, &mut host_writer),
                )
                .await;
            drop((host_reader, host_writer));
            report
        };

        let caller = async {
            let accept = caller_reader.next().await?.ok_or("no accept")?;
            assert_eq!(accept.kind(), FrameKind::Accept);
            let mut caller_read = caller_reader.into_inner();
            let mut head = [0_u8; 8];
            caller_read.read_exact(&mut head).await?;
            if send_cancel {
                caller_writer.frame(FrameKind::Cancel, &[]).await?;
                caller_writer.flush().await?;
            } else {
                caller_writer.get_mut().shutdown().await?;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let mut bytes = head.to_vec();
            caller_read.read_to_end(&mut bytes).await?;
            let mut frames = FrameReader::new(bytes.as_slice());
            let mut data_frames = 0_usize;
            let mut outcome = None;
            while let Some(frame) = frames.next().await? {
                match frame.kind() {
                    FrameKind::Data => data_frames += 1,
                    FrameKind::Outcome => {
                        outcome = Some(AgentReply::decode_outcome(frame.payload())?);
                    }
                    other => return Err(format!("unexpected frame {other:?}").into()),
                }
            }
            TestResult::Ok((data_frames, outcome))
        };
        let (report, caller_res) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(watched, caller)
        })
        .await?;
        let (data_frames, outcome) = caller_res?;
        live.finish().await?;
        assert_eq!(
            report.end,
            ma2a_services::OperationEnd::Failed(ServiceError::Cancelled)
        );
        assert_eq!(
            data_frames, 1,
            "watch kept sending {data_frames} events after caller cancel/EOF"
        );
        assert_eq!(outcome, Some(Err(ServiceError::Cancelled)));
    }
    Ok(())
}
