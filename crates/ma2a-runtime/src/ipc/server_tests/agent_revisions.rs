//! Local Agent mutations answer at the revision of their own commit.

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentMessage, MAX_LOG_LIMIT, MessageRole, ProcessId, ProcessSpec, ProcessStart,
    },
};
use ma2a_store::{AgentDefinition, MutationReplayState, NewProcess, Repository};
use serde_json::json;

use super::{services::call, *};

struct ServedAgent {
    state: TempState,
    config: StoreConfig,
    runtime: Runtime,
    server: LiveServer,
    paths: IpcPaths,
    client: LocalApiClient,
}

impl ServedAgent {
    async fn start(name: &str) -> TestResult<Self> {
        let state = TempState::new_named(name)?;
        let config = StoreConfig::new(&state.0);
        let runtime = Runtime::start(config.clone()).await?;
        let control = CurrentUserRuntime::open_at(
            &state.0,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        let paths = IpcPaths::new(&state.0)?;
        let server = LiveServer::spawn(
            LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?,
        );
        let client = LocalApiClient::new(paths.clone());
        Ok(Self {
            state,
            config,
            runtime,
            server,
            paths,
            client,
        })
    }

    async fn stop(self) -> TestResult {
        self.server.cancel().await?;
        self.paths.remove_stale_endpoint()?;
        self.runtime.shutdown().await?;
        drop(self.state);
        Ok(())
    }
}

fn define_helper(config: &StoreConfig, name: &str) -> TestResult<AgentId> {
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let agent = AgentId::new(name)?;
    let definition = AgentDefinition {
        id: agent.clone(),
        description: String::new(),
        start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
        tools: 0,
        max_tasks: 1,
    };
    Repository::open(config)?.define_agent(&definition, 1)?;
    Ok(agent)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_start_answers_at_the_revision_that_started_it() -> TestResult {
    // Given: a defined Agent, and a record of the revision each change of its state commits.
    let served = ServedAgent::start("agent-revision").await?;
    define_helper(&served.config, "helper")?;
    let sql = rusqlite::Connection::open(served.config.database_path())?;
    sql.execute_batch(
        "CREATE TABLE test_enabled (revision INTEGER NOT NULL);
         CREATE TRIGGER test_enabled AFTER UPDATE OF enabled ON agents
         BEGIN INSERT INTO test_enabled SELECT revision + 1 FROM runtime_metadata; END;",
    )?;

    // When: the Agent is started, which also begins its first instance.
    let started = call(
        &served.client,
        json!({
            "version": 1, "operation": "agent_start", "request_id": "71".repeat(16),
            "agent": "helper",
        }),
    )
    .await?;

    // Then: the answer carries the start's own commit, not a later one.
    let committed: u64 =
        sql.query_row("SELECT revision FROM test_enabled", [], |row| row.get(0))?;
    served.stop().await?;
    assert_eq!(started.get("error"), None, "{started}");
    assert_eq!(
        started.get("revision").and_then(serde_json::Value::as_u64),
        Some(committed)
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_start_counts_the_tasks_live_at_its_own_commit() -> TestResult {
    // Given: a defined Agent, and a task that appears only once its first instance begins,
    // after the start committed.
    let served = ServedAgent::start("agent-live").await?;
    define_helper(&served.config, "helper")?;
    let input = AgentMessage::text(MessageRole::User, "hello")?.to_bytes()?;
    rusqlite::Connection::open(served.config.database_path())?.execute_batch(&format!(
        "CREATE TRIGGER test_late_task AFTER INSERT ON agent_instances
         BEGIN INSERT INTO agent_tasks (task_id, agent_id, creator_endpoint_id, request_id,
             fingerprint, state, input, last_seq, created_at_ms)
         VALUES (randomblob(16), 'helper', zeroblob(32), randomblob(16), zeroblob(32), 0,
             X'{}', 0, 1); END;",
        api::encode_hex(&input)
    ))?;

    // When
    let started = call(
        &served.client,
        json!({
            "version": 1, "operation": "agent_start", "request_id": "72".repeat(16),
            "agent": "helper",
        }),
    )
    .await?;

    // Then: the answer counts the tasks live when the start committed, not the later one.
    served.stop().await?;
    assert_eq!(started.get("error"), None, "{started}");
    assert_eq!(
        started.pointer("/result/payload/live_tasks"),
        Some(&json!(0)),
        "{started}"
    );
    Ok(())
}

fn seed_bound_agent_process(
    (config, local): (&StoreConfig, ma2a_core::EndpointId),
    (name, pid_byte): (&str, u8),
) -> TestResult<(AgentId, ProcessId)> {
    let agent = define_helper(config, name)?;
    let mut repository = Repository::open(config)?;
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?
        .generation;
    let process_id = ProcessId::from_bytes([pid_byte; 16]);
    repository.create_process(&NewProcess {
        id: process_id,
        binding: (local, RequestId::random()?),
        fingerprint: [pid_byte.wrapping_add(0x10); 32],
        start: (&start_bytes, None),
        secret: [pid_byte.wrapping_add(0x20); 32],
        created_ms: 4,
    })?;
    repository.bind_agent_process((&agent, generation), process_id)?;
    Ok((agent, process_id))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stop_post_commit_termination_failure_records_committed_revision() -> TestResult {
    // Given: an enabled Agent bound to a Process in `created` state (so termination fails
    // after `agent_stop` commits `enabled = false`), plus a trigger recording the commit revision.
    let served = ServedAgent::start("agent-stop-rev").await?;
    let local = served.runtime.handle().status().await?.endpoint_id();
    let (_agent, _process_id) =
        seed_bound_agent_process((&served.config, local), ("stop-fail", 0x27))?;

    let sql = rusqlite::Connection::open(served.config.database_path())?;
    sql.execute_batch(
        "CREATE TABLE test_disabled (revision INTEGER NOT NULL);
         CREATE TRIGGER test_disabled AFTER UPDATE OF enabled ON agents
         BEGIN INSERT INTO test_disabled SELECT revision + 1 FROM runtime_metadata; END;",
    )?;

    // When: `agent_stop` commits `enabled = false` and then fails to terminate the `created`
    // Process, followed by an exact retry of the same request ID.
    let request_hex = "73".repeat(16);
    let stop_cmd = json!({
        "version": 1, "operation": "agent_stop", "request_id": request_hex,
        "agent": "stop-fail",
    });
    let stop_reply = call(&served.client, stop_cmd.clone()).await?;
    let replay_reply = call(&served.client, stop_cmd).await?;

    let committed: u64 =
        sql.query_row("SELECT revision FROM test_disabled", [], |row| row.get(0))?;
    let request_id = RequestId::try_from([0x73_u8; 16].as_slice())?;
    let replay_state = Repository::open(&served.config)?.mutation_replay(request_id)?;
    served.stop().await?;

    // Then: the failure reports a committed mutation, the replay record keeps the committed
    // revision (not the stale pre-commit one), and an exact retry replays that outcome.
    assert_eq!(
        stop_reply.get("error").and_then(serde_json::Value::as_str),
        Some("unavailable"),
        "{stop_reply}"
    );
    assert_eq!(
        stop_reply
            .get("remediation")
            .and_then(serde_json::Value::as_str),
        Some(
            "mutation committed; completion failed; inspect current state; this request will not execute again"
        ),
        "{stop_reply}"
    );
    assert_eq!(replay_reply, stop_reply);
    let Some(MutationReplayState::Completed(entry)) = replay_state else {
        return Err(format!("expected completed replay entry, got {replay_state:?}").into());
    };
    assert_eq!(entry.revision(), committed);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_undefine_while_previous_process_runs_has_no_effect_and_retries() -> TestResult {
    use ma2a_core::service::{ProcessState, Termination};
    use ma2a_store::ProcessTransition;

    // Given: a stopped Agent whose previous instance points at a live Running Process.
    let served = ServedAgent::start("agent-undefine-live").await?;
    let local = served.runtime.handle().status().await?.endpoint_id();
    let (agent, process_id) =
        seed_bound_agent_process((&served.config, local), ("undefine-live", 0x28))?;
    {
        let mut repository = Repository::open(&served.config)?;
        for (to, at_ms) in [(ProcessState::Starting, 5), (ProcessState::Running, 6)] {
            repository.transition_process(
                process_id,
                ProcessTransition {
                    to,
                    reason: None,
                    termination: None,
                    at_ms,
                },
            )?;
        }
        repository.set_agent_enabled(&agent, (false, 7))?;
    }

    // When: `agent_undefine` runs while the previous Process is Running, then the Process exits
    // and the exact same request ID is retried.
    let request_hex = "74".repeat(16);
    let cmd = json!({
        "version": 1, "operation": "agent_undefine", "request_id": request_hex,
        "agent": "undefine-live",
    });
    let refused = call(&served.client, cmd.clone()).await?;
    let request_id = RequestId::try_from([0x74_u8; 16].as_slice())?;
    let replay_before_retry = Repository::open(&served.config)?.mutation_replay(request_id)?;
    let state_after_refusal = Repository::open(&served.config)?
        .process(process_id)?
        .map(|record| record.state);
    Repository::open(&served.config)?.transition_process(
        process_id,
        ProcessTransition {
            to: ProcessState::Exited,
            reason: None,
            termination: Some(Termination::Exited(0)),
            at_ms: 8,
        },
    )?;
    let retried = call(&served.client, cmd).await?;
    served.stop().await?;

    assert_eq!(
        refused.get("error").and_then(serde_json::Value::as_str),
        Some("unavailable"),
        "{refused}"
    );
    assert_eq!(state_after_refusal, Some(ProcessState::Running));
    assert_eq!(replay_before_retry, None);
    assert_eq!(retried.get("error"), None, "{retried}");
    assert_eq!(
        retried
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str),
        Some("agent_undefined")
    );
    Ok(())
}
