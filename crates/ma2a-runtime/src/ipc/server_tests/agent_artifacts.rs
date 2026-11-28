//! Local IPC `agent_task_artifact_read` returns `internal` when a recorded blob is missing,
//! wrong-sized, or not a regular file.

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentMessage, AgentTaskId, ArtifactRef, MAX_LOG_LIMIT, MessageRole, ProcessSpec,
        ProcessStart,
    },
};
use ma2a_store::{AgentDefinition, AgentTaskCreation, NewAgentTask, Repository};
use serde_json::json;

use super::{services::call, *};

struct ArtifactFixture {
    state: TempState,
    runtime: Runtime,
    server: LiveServer,
    paths: IpcPaths,
    client: LocalApiClient,
    task: AgentTaskId,
    blobs: std::path::PathBuf,
}

impl ArtifactFixture {
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
        let local = runtime.handle().status().await?.endpoint_id();
        let mut repository = Repository::open(&config)?;
        let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
        let agent = AgentId::new("helper")?;
        repository.define_agent(
            &AgentDefinition {
                id: agent.clone(),
                description: String::new(),
                start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?;
        repository.set_agent_enabled(&agent, (true, 2))?;
        let task = AgentTaskId::random()?;
        let AgentTaskCreation::Created(_) = repository
            .create_agent_task(&NewAgentTask {
                id: task,
                agent,
                binding: (local, RequestId::random()?),
                fingerprint: [0x11; 32],
                from_agent: None,
                input: AgentMessage::text(MessageRole::User, "work")?,
                created_ms: 2,
            })?
            .into_value()
        else {
            return Err("task was not created".into());
        };
        let blobs = state.0.join("agents-v1").join("blobs");
        std::fs::create_dir_all(&blobs)?;
        Ok(Self {
            state,
            runtime,
            server,
            paths,
            client,
            task,
            blobs,
        })
    }

    fn record_artifact(&self, name: &str, content: &[u8]) -> TestResult<std::path::PathBuf> {
        let digest = *blake3::hash(content).as_bytes();
        let artifact = ArtifactRef::new(
            (name, "text/plain"),
            (u64::try_from(content.len())?, digest),
        )?;
        Repository::open(&StoreConfig::new(&self.state.0))?
            .add_agent_artifact(self.task, (&artifact, 3))?;
        Ok(self.blobs.join(api::encode_hex(&digest)))
    }

    async fn read_artifact(&self, name: &str) -> TestResult<serde_json::Value> {
        call(
            &self.client,
            json!({
                "version": 1,
                "operation": "agent_task_artifact_read",
                "task_id": self.task.to_string(),
                "name": name,
                "offset": 0,
            }),
        )
        .await
    }

    async fn stop(self) -> TestResult {
        self.server.cancel().await?;
        self.paths.remove_stale_endpoint()?;
        self.runtime.shutdown().await?;
        drop(self.state);
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_task_artifact_read_reports_internal_for_missing_and_wrong_size_blobs() -> TestResult
{
    // Given: recorded artifacts whose blob file is missing or has the wrong size.
    let fixture = ArtifactFixture::start("agent-artifact-err").await?;
    let _missing_path = fixture.record_artifact("missing.txt", b"expected bytes")?;
    let wrong_size_path = fixture.record_artifact("truncated.txt", b"expected bytes")?;
    std::fs::write(&wrong_size_path, b"short")?;

    // When
    let missing_reply = fixture.read_artifact("missing.txt").await?;
    let wrong_size_reply = fixture.read_artifact("truncated.txt").await?;
    fixture.stop().await?;

    // Then
    assert_eq!(
        missing_reply
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some("internal"),
        "{missing_reply}"
    );
    assert_eq!(
        wrong_size_reply
            .get("error")
            .and_then(serde_json::Value::as_str),
        Some("internal"),
        "{wrong_size_reply}"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_task_artifact_read_reports_internal_for_symlink_blob() -> TestResult {
    // Given: a recorded artifact whose blob path is a symlink to matching-length content.
    let fixture = ArtifactFixture::start("agent-artifact-link").await?;
    let content = b"unrelated secret";
    let blob_path = fixture.record_artifact("link.txt", content)?;
    let target = fixture.state.0.join("target");
    std::fs::write(&target, content)?;
    std::os::unix::fs::symlink(&target, &blob_path)?;

    // When
    let reply = fixture.read_artifact("link.txt").await?;
    fixture.stop().await?;

    // Then
    assert_eq!(
        reply.get("error").and_then(serde_json::Value::as_str),
        Some("internal"),
        "{reply}"
    );
    Ok(())
}
