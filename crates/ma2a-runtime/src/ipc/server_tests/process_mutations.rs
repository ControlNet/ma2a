//! Blocking Process/Agent mutations release the replay lock after reserving their `RequestId`.
#![cfg(unix)]

use std::time::Duration;

use ma2a_core::{
    RequestId,
    service::{AgentId, MAX_LOG_LIMIT, ProcessReason, ProcessSpec, ProcessStart, ProcessState},
};
use ma2a_store::{AgentDefinition, Repository};
use serde_json::json;
use tokio::time::timeout;

use super::{services::call, *};
use crate::processes::ProcessHosting;

const PROMPT: Duration = Duration::from_secs(2);

fn error_name(reply: &serde_json::Value) -> Option<&str> {
    reply.get("error").and_then(serde_json::Value::as_str)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn process_terminate_does_not_block_unrelated_mutations_or_pending_replay() -> TestResult {
    // Given: a Runtime with a running local Process that ignores SIGTERM.
    let state = TempState::new_named("proc-replay")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start_with_processes(config, ProcessHosting::in_process()).await?;
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
    let handle = runtime.handle();
    let manager = handle.processes().ok_or("no Processes")?;
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec!["-c".to_owned(), "trap '' TERM; sleep 60".to_owned()],
        Some("/".to_owned()),
    )?;
    let started = manager
        .start_local(
            RequestId::try_from([0x61; 16].as_slice())?,
            ProcessStart::new(spec, false, MAX_LOG_LIMIT)?,
        )
        .await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if manager.local_record(started.id).await?.map(|r| r.state) == Some(ProcessState::Running) {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let terminate_cmd = json!({
        "version": 1,
        "operation": "process_terminate",
        "request_id": "a1".repeat(16),
        "process_id": started.id.to_string(),
        "grace_ms": 10_000,
    });
    let create_cmd = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2","name":"during-term"}"#,
    )?;

    // When: process_terminate waits on the 10 s grace period.
    let client_a = LocalApiClient::new(paths.clone());
    let terminate_req = terminate_cmd.clone();
    let terminating = tokio::spawn(async move { call(&client_a, terminate_req).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Then: an unrelated mutation and a retry of the pending RequestId both answer promptly.
    let during_term =
        timeout(PROMPT, LocalApiClient::new(paths.clone()).call(&create_cmd)).await??;
    let during_term: serde_json::Value = serde_json::from_slice(&during_term)?;
    assert_eq!(error_name(&during_term), None, "{during_term}");

    let pending_retry = timeout(
        PROMPT,
        call(&LocalApiClient::new(paths.clone()), terminate_cmd),
    )
    .await??;
    assert_eq!(
        error_name(&pending_retry),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{pending_retry}"
    );
    assert!(
        !terminating.is_finished(),
        "terminate must still be waiting"
    );

    let _ended = manager
        .terminate((None, started.id), (50, ProcessReason::Requested))
        .await;
    let _terminated = timeout(Duration::from_secs(10), terminating).await??;
    assert_eq!(server.cancel().await?, ServerExit::Cancelled);
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_stop_does_not_block_unrelated_mutations_or_pending_replay() -> TestResult {
    // Given: a started Agent whose instance Process ignores SIGTERM.
    let state = TempState::new_named("agent-replay")?;
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
        vec!["-c".to_owned(), "trap '' TERM; sleep 60".to_owned()],
        Some("/".to_owned()),
    )?;
    let agent = AgentId::new("slow-stop")?;
    let definition = AgentDefinition {
        id: agent.clone(),
        description: String::new(),
        start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
        tools: 0,
        max_tasks: 1,
    };
    Repository::open(&config)?.define_agent(&definition, 1)?;
    let client = LocalApiClient::new(paths.clone());
    let started = call(
        &client,
        json!({
            "version": 1, "operation": "agent_start", "request_id": "b1".repeat(16),
            "agent": "slow-stop",
        }),
    )
    .await?;
    assert_eq!(error_name(&started), None, "{started}");

    let handle = runtime.handle();
    let processes = handle.processes().ok_or("no Processes")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let process_id = loop {
        if let Some(pid) = Repository::open(&config)?
            .agent(&agent)?
            .and_then(|record| record.instance)
            .and_then(|instance| instance.process_id)
            && processes
                .local_record(pid)
                .await?
                .is_some_and(|r| r.state == ProcessState::Running)
        {
            break pid;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let stop_cmd = json!({
        "version": 1, "operation": "agent_stop", "request_id": "b2".repeat(16),
        "agent": "slow-stop",
    });
    let create_cmd = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3b3","name":"during-stop"}"#,
    )?;

    // When: agent_stop waits on STOP_GRACE_MS (5 s).
    let client_a = LocalApiClient::new(paths.clone());
    let stop_req = stop_cmd.clone();
    let stopping = tokio::spawn(async move { call(&client_a, stop_req).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Then: an unrelated mutation and a pending retry both answer promptly.
    let during_stop =
        timeout(PROMPT, LocalApiClient::new(paths.clone()).call(&create_cmd)).await??;
    let during_stop: serde_json::Value = serde_json::from_slice(&during_stop)?;
    assert_eq!(error_name(&during_stop), None, "{during_stop}");

    let pending_retry = timeout(PROMPT, call(&client, stop_cmd)).await??;
    assert_eq!(
        error_name(&pending_retry),
        Some(ProtocolError::UNAVAILABLE.name()),
        "{pending_retry}"
    );
    assert!(!stopping.is_finished(), "agent_stop must still be waiting");

    let _ended = processes
        .terminate((None, process_id), (50, ProcessReason::Requested))
        .await;
    let _stopped = timeout(Duration::from_secs(10), stopping).await??;
    assert_eq!(server.cancel().await?, ServerExit::Cancelled);
    paths.remove_stale_endpoint()?;
    runtime.shutdown().await?;
    Ok(())
}
