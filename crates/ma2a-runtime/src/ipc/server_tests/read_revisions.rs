//! Local Process and Agent reads answer at the revision of the Store read that made them,
//! never at an older one the Runtime held when the request arrived.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentId, AgentMessage, MAX_LOG_LIMIT, MessageRole, ProcessId, ProcessSpec, ProcessStart,
    },
};
use ma2a_store::{AgentDefinition, NewProcess, Repository};
use serde_json::json;

use super::{services::call, *};

fn seed_external_commits(state: &std::path::Path, task: &str) -> TestResult<u64> {
    let config = StoreConfig::new(state);
    let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
    let start = ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?;
    let mut repository = Repository::open(&config)?;
    let creator: EndpointId = iroh::SecretKey::generate().public().into();
    repository.create_process(&NewProcess {
        id: ProcessId::from_bytes([0x31; 16]),
        binding: (creator, RequestId::try_from([0x32; 16].as_slice())?),
        fingerprint: [0x33; 32],
        start: (&start, None),
        secret: [0x34; 32],
        created_ms: 1_000,
    })?;
    repository.define_agent(
        &AgentDefinition {
            id: AgentId::new("helper")?,
            description: String::new(),
            start,
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    let input = AgentMessage::text(MessageRole::User, "hello")?.to_bytes()?;
    let content = b"artifact-bytes";
    let digest = blake3::hash(content);
    let blobs = state.join("agents-v1").join("blobs");
    std::fs::create_dir_all(&blobs)?;
    std::fs::write(blobs.join(digest.to_hex().as_str()), content)?;
    let sql = rusqlite::Connection::open(config.database_path())?;
    sql.execute_batch(&format!(
        "INSERT INTO agent_tasks (task_id, agent_id, creator_endpoint_id, request_id,
             fingerprint, state, input, last_seq, created_at_ms)
         VALUES (X'{task}', 'helper', zeroblob(32), randomblob(16), zeroblob(32), 0,
             X'{}', 0, 1);
         INSERT INTO agent_artifacts (task_id, name, position, media_type, size, blake3)
         VALUES (X'{task}', 'note.txt', 0, 'text/plain', {}, X'{}');
         UPDATE runtime_metadata SET revision = revision + 1;",
        api::encode_hex(&input),
        content.len(),
        digest.to_hex()
    ))?;
    Ok(
        sql.query_row("SELECT revision FROM runtime_metadata", [], |row| {
            row.get(0)
        })?,
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_and_agent_reads_answer_at_the_revision_they_read() -> TestResult {
    // Given: a Process, an Agent task, and Store commits the Runtime itself did not make, as
    // the Process and Agent hosts make them.
    let state = TempState::new_named("read-revisions")?;
    let runtime = Runtime::start(StoreConfig::new(&state.0)).await?;
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
    let before = runtime.handle().status().await?.revision();
    let task = "35".repeat(16);
    let committed = seed_external_commits(&state.0, &task)?;
    assert!(before < committed);

    // When
    let reads = [
        json!({"version": 1, "operation": "process_list", "after": null}),
        json!({"version": 1, "operation": "process_inspect", "process_id": "31".repeat(16)}),
        json!({"version": 1, "operation": "agent_task_inspect", "task_id": task}),
        json!({"version": 1, "operation": "agent_task_events", "task_id": task, "after": 0}),
        json!({"version": 1, "operation": "agent_task_artifact_read", "task_id": task, "name": "note.txt", "offset": 0}),
        json!({"version": 1, "operation": "status"}),
        json!({"version": 1, "operation": "endpoint_info"}),
        json!({"version": 1, "operation": "control_sync_status", "peer_endpoint_id": PEER_ENDPOINT_ID}),
        json!({"version": 1, "operation": "operations_status"}),
        json!({"version": 1, "operation": "ui_status"}),
    ];
    let mut answers = Vec::new();
    for read in reads {
        answers.push(call(&client, read).await?);
    }

    // Then: each is stamped with a revision at least as new as those commits.
    server.cancel().await?;
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    for answer in answers {
        assert_eq!(answer.get("error"), None, "{answer}");
        let revision = answer.get("revision").and_then(serde_json::Value::as_u64);
        assert!(
            revision.is_some_and(|revision| revision >= committed),
            "{answer}"
        );
        if answer
            .pointer("/result/type")
            .and_then(serde_json::Value::as_str)
            == Some("status")
        {
            let payload_revision = answer
                .pointer("/result/payload/revision")
                .and_then(serde_json::Value::as_u64);
            assert!(
                payload_revision.is_some_and(|revision| revision >= committed),
                "{answer}"
            );
        }
    }
    Ok(())
}
