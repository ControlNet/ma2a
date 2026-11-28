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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_start_answers_at_the_revision_that_started_it() -> TestResult {
    // Given: a defined Agent, and a record of the revision each change of its state commits.
    let state = TempState::new_named("agent-revision")?;
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
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let definition = AgentDefinition {
        id: AgentId::new("helper")?,
        description: String::new(),
        start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
        tools: 0,
        max_tasks: 1,
    };
    Repository::open(&config)?.define_agent(&definition, 1)?;
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TABLE test_enabled (revision INTEGER NOT NULL);
         CREATE TRIGGER test_enabled AFTER UPDATE OF enabled ON agents
         BEGIN INSERT INTO test_enabled SELECT revision + 1 FROM runtime_metadata; END;",
    )?;

    // When: the Agent is started, which also begins its first instance.
    let started = call(
        &client,
        json!({
            "version": 1, "operation": "agent_start", "request_id": "71".repeat(16),
            "agent": "helper",
        }),
    )
    .await?;

    // Then: the answer carries the start's own commit, not a later one.
    let committed: u64 =
        sql.query_row("SELECT revision FROM test_enabled", [], |row| row.get(0))?;
    server.cancel().await?;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
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
    let state = TempState::new_named("agent-live")?;
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
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let definition = AgentDefinition {
        id: AgentId::new("helper")?,
        description: String::new(),
        start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
        tools: 0,
        max_tasks: 1,
    };
    Repository::open(&config)?.define_agent(&definition, 1)?;
    let input = AgentMessage::text(MessageRole::User, "hello")?.to_bytes()?;
    rusqlite::Connection::open(config.database_path())?.execute_batch(&format!(
        "CREATE TRIGGER test_late_task AFTER INSERT ON agent_instances
         BEGIN INSERT INTO agent_tasks (task_id, agent_id, creator_endpoint_id, request_id,
             fingerprint, state, input, last_seq, created_at_ms)
         VALUES (randomblob(16), 'helper', zeroblob(32), randomblob(16), zeroblob(32), 0,
             X'{}', 0, 1); END;",
        api::encode_hex(&input)
    ))?;

    // When
    let started = call(
        &client,
        json!({
            "version": 1, "operation": "agent_start", "request_id": "72".repeat(16),
            "agent": "helper",
        }),
    )
    .await?;

    // Then: the answer counts the tasks live when the start committed, not the later one.
    server.cancel().await?;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    assert_eq!(started.get("error"), None, "{started}");
    assert_eq!(
        started.pointer("/result/payload/live_tasks"),
        Some(&json!(0)),
        "{started}"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stop_post_commit_termination_failure_records_committed_revision() -> TestResult {
    // Given: an enabled Agent bound to a Process in `created` state (so termination fails
    // after `agent_stop` commits `enabled = false`), plus a trigger recording the commit revision.
    let state = TempState::new_named("agent-stop-rev")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let local = runtime.handle().status().await?.endpoint_id();
    let mut repository = Repository::open(&config)?;
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start_bytes = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let agent = AgentId::new("stop-fail")?;
    repository.define_agent(
        &AgentDefinition {
            id: agent.clone(),
            description: String::new(),
            start: start_bytes.clone(),
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    repository.set_agent_enabled(&agent, (true, 2))?;
    let generation = repository
        .begin_agent_instance(&agent, 3)?
        .into_value()
        .and_then(|record| record.instance)
        .ok_or("no instance began")?
        .generation;
    let process_id = ProcessId::from_bytes([0x27; 16]);
    repository.create_process(&NewProcess {
        id: process_id,
        binding: (local, RequestId::random()?),
        fingerprint: [0x37; 32],
        start: (&start_bytes, None),
        secret: [0x47; 32],
        created_ms: 4,
    })?;
    repository.bind_agent_process((&agent, generation), process_id)?;
    drop(repository);

    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(
        "CREATE TABLE test_disabled (revision INTEGER NOT NULL);
         CREATE TRIGGER test_disabled AFTER UPDATE OF enabled ON agents
         BEGIN INSERT INTO test_disabled SELECT revision + 1 FROM runtime_metadata; END;",
    )?;

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

    // When: `agent_stop` commits `enabled = false` and then fails to terminate the `created`
    // Process, followed by an exact retry of the same request ID.
    let request_hex = "73".repeat(16);
    let stop_cmd = json!({
        "version": 1, "operation": "agent_stop", "request_id": request_hex,
        "agent": "stop-fail",
    });
    let stop_reply = call(&client, stop_cmd.clone()).await?;
    let replay_reply = call(&client, stop_cmd).await?;

    let committed: u64 =
        sql.query_row("SELECT revision FROM test_disabled", [], |row| row.get(0))?;
    let request_id = RequestId::try_from([0x73_u8; 16].as_slice())?;
    let replay_state = Repository::open(&config)?.mutation_replay(request_id)?;
    server.cancel().await?;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;

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
