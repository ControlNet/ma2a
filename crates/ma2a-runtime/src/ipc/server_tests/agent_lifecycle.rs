//! Concurrent Agent lifecycle operations serialize per Agent through termination without
//! blocking unrelated durable mutations on the global replay lock.
#![cfg(unix)]

use std::{path::Path, time::Duration};

use ma2a_core::service::{
    AgentId, MAX_LOG_LIMIT, ProcessId, ProcessReason, ProcessSpec, ProcessStart, ProcessState,
};
use ma2a_store::{AgentDefinition, AgentInstanceRecord, Repository};
use serde_json::json;
use tokio::time::timeout;

use super::{services::call, *};
use crate::processes::{ProcessHosting, ProcessManager};

const PROMPT: Duration = Duration::from_secs(2);

fn error_name(reply: &serde_json::Value) -> Option<&str> {
    reply.get("error").and_then(serde_json::Value::as_str)
}

async fn wait_running_instance(
    (config, processes): (&StoreConfig, &ProcessManager),
    (agent, pids_path): (&AgentId, &Path),
    (expected_pids, excluding): (usize, Option<ProcessId>),
) -> Result<ProcessId, Box<dyn std::error::Error + Send + Sync>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(pid) = Repository::open(config)?
            .agent(agent)?
            .and_then(|record| record.instance)
            .filter(AgentInstanceRecord::is_live)
            .and_then(|instance| instance.process_id)
            && Some(pid) != excluding
            && processes
                .local_record(pid)
                .await?
                .is_some_and(|r| r.state == ProcessState::Running)
            && std::fs::read_to_string(pids_path)
                .is_ok_and(|contents| contents.lines().count() == expected_pids)
        {
            return Ok(pid);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "instance did not start (expected {expected_pids} PIDs)"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct ContestedFixture {
    state: TempState,
    config: StoreConfig,
    runtime: Runtime,
    server: LiveServer,
    paths: IpcPaths,
    agent: AgentId,
    pids_path: std::path::PathBuf,
    first_process: ProcessId,
}

async fn start_contested_agent()
-> Result<ContestedFixture, Box<dyn std::error::Error + Send + Sync>> {
    let state = TempState::new_named("agent-stop-start")?;
    let config = StoreConfig::new(&state.0);
    let runtime =
        Runtime::start_with_processes(config.clone(), ProcessHosting::in_process()).await?;
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
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec![
            "-c".to_owned(),
            "trap '' TERM; printf '%s\\n' $$ >> \"$MA2A_AGENT_WORKSPACE/pids\"; sleep 60"
                .to_owned(),
        ],
        None,
    )?;
    let agent = AgentId::new("contested")?;
    Repository::open(&config)?.define_agent(
        &AgentDefinition {
            id: agent.clone(),
            description: String::new(),
            start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
            tools: 0,
            max_tasks: 1,
        },
        1,
    )?;
    let client = LocalApiClient::new(paths.clone());
    let started = call(
        &client,
        json!({
            "version": 1, "operation": "agent_start", "request_id": "c1".repeat(16),
            "agent": "contested",
        }),
    )
    .await?;
    assert_eq!(error_name(&started), None, "{started}");

    let handle = runtime.handle();
    let processes = handle.processes().ok_or("no Processes")?;
    let pids_path = state
        .0
        .join("agents-v1")
        .join("contested")
        .join("workspace")
        .join("pids");
    let first_process =
        wait_running_instance((&config, processes), (&agent, &pids_path), (1, None)).await?;
    Ok(ContestedFixture {
        state,
        config,
        runtime,
        server,
        paths,
        agent,
        pids_path,
        first_process,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_start_waits_for_concurrent_agent_stop_termination() -> TestResult {
    // Given: a started Agent whose instance ignores SIGTERM and records its PID in its workspace.
    let fixture = start_contested_agent().await?;
    let stop_cmd = json!({
        "version": 1, "operation": "agent_stop", "request_id": "c2".repeat(16),
        "agent": "contested",
    });
    let start_cmd = json!({
        "version": 1, "operation": "agent_start", "request_id": "c3".repeat(16),
        "agent": "contested",
    });
    let create_cmd = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4","name":"during-stop-start"}"#,
    )?;

    // When: agent_stop is waiting on STOP_GRACE_MS (5 s) and agent_start arrives for the same Agent.
    let stop_client = LocalApiClient::new(fixture.paths.clone());
    let stopping = tokio::spawn(async move { call(&stop_client, stop_cmd).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let start_client = LocalApiClient::new(fixture.paths.clone());
    let starting = tokio::spawn(async move { call(&start_client, start_cmd).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Then: unrelated mutations stay responsive, and the new start waits until the old child
    // finishes terminating rather than running a second child concurrently.
    let unrelated = timeout(
        PROMPT,
        LocalApiClient::new(fixture.paths.clone()).call(&create_cmd),
    )
    .await??;
    let unrelated: serde_json::Value = serde_json::from_slice(&unrelated)?;
    assert_eq!(error_name(&unrelated), None, "{unrelated}");
    assert!(
        !stopping.is_finished(),
        "agent_stop must still be terminating"
    );
    assert!(
        !starting.is_finished(),
        "agent_start must serialize behind agent_stop termination"
    );
    let recorded_pids = std::fs::read_to_string(&fixture.pids_path)?;
    assert_eq!(
        recorded_pids.lines().count(),
        1,
        "a second child started while the first was still alive: {recorded_pids}"
    );

    let stop_reply = timeout(Duration::from_secs(10), stopping).await???;
    let start_reply = timeout(Duration::from_secs(10), starting).await???;
    assert_eq!(error_name(&stop_reply), None, "{stop_reply}");
    assert_eq!(error_name(&start_reply), None, "{start_reply}");

    let handle = fixture.runtime.handle();
    let processes = handle.processes().ok_or("no Processes")?;
    let second_process = wait_running_instance(
        (&fixture.config, processes),
        (&fixture.agent, &fixture.pids_path),
        (2, Some(fixture.first_process)),
    )
    .await?;
    let first_record = processes
        .local_record(fixture.first_process)
        .await?
        .ok_or("first process missing")?;
    let second_record = processes
        .local_record(second_process)
        .await?
        .ok_or("second process missing")?;
    assert_eq!(first_record.state, ProcessState::Terminated);
    let first_ended = first_record
        .times
        .ended_ms
        .ok_or("first process missing ended_ms")?;
    assert!(
        second_record.times.created_ms >= first_ended,
        "replacement created ({}) before first ended ({first_ended})",
        second_record.times.created_ms
    );

    let _ended = processes
        .terminate((None, second_process), (50, ProcessReason::Requested))
        .await;
    assert_eq!(fixture.server.cancel().await?, ServerExit::Cancelled);
    fixture.paths.remove_stale_endpoint()?;
    fixture.runtime.shutdown().await?;
    drop(fixture.state);
    Ok(())
}
