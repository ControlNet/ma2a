//! `ListTasks` parameter validation, pagination, and filter consistency across detail reads.

use ma2a_core::service::AgentId;
use ma2a_runtime::ipc::IpcPaths;
use serde_json::{Value, json};

use super::list;
use crate::a2a::{
    backend::{Backend, Route},
    view::Served,
};

/// The error code `ListTasks` answers `params` with, through a Runtime that is not
/// running: a parameter it refuses answers `-32602` before any task is read.
async fn code(params: Value) -> Result<i64, Box<dyn std::error::Error>> {
    let state_dir = std::env::temp_dir().join("ma2a-a2a-list-no-runtime");
    let backend = Backend {
        paths: IpcPaths::new(&state_dir)?,
        state_dir,
    };
    let route = Route::Local(AgentId::new("lead")?);
    let served = Served {
        backend: &backend,
        route: &route,
        base: "http://127.0.0.1",
    };
    let listed = list(served, &params).await;
    Ok(listed.err().ok_or("listed without a Runtime")?.code)
}

#[tokio::test]
async fn a_filter_or_token_that_is_not_a_string_is_refused()
-> Result<(), Box<dyn std::error::Error>> {
    for name in ["contextId", "status", "statusTimestampAfter", "pageToken"] {
        assert_eq!(code(json!({name: 7})).await?, -32_602, "{name}");
    }
    // Absent, null, or empty names nothing, so the listing goes on to read tasks.
    assert_eq!(
        code(json!({"contextId": null, "pageToken": ""})).await?,
        -32_603
    );
    Ok(())
}

#[test]
fn a_task_is_kept_by_the_time_of_its_last_state_change() {
    // Started at 1 s, waiting since 5 s; the filter's bound is inclusive.
    let task = json!({"task_id": "t", "state": "waiting", "created_at_ms": 500,
        "started_at_ms": 1_000, "ended_at_ms": null, "changed_at_ms": 5_000});
    let after = |changed_after_ms| super::Filter {
        context: None,
        status: None,
        changed_after_ms: Some(changed_after_ms),
    };
    assert!(after(3_000).keeps(&task) && after(5_000).keeps(&task));
    assert!(!after(5_001).keeps(&task));
}

#[tokio::test]
async fn include_artifacts_is_a_boolean() -> Result<(), Box<dyn std::error::Error>> {
    for flag in [json!("true"), json!(1)] {
        assert_eq!(code(json!({"includeArtifacts": flag})).await?, -32_602);
    }
    assert_eq!(code(json!({"includeArtifacts": true})).await?, -32_603);
    assert_eq!(code(json!({"includeArtifacts": null})).await?, -32_603);
    Ok(())
}

#[tokio::test]
async fn a_page_token_is_a_task_id_as_listed() -> Result<(), Box<dyn std::error::Error>> {
    // Task IDs are listed in lowercase, so an uppercase token would sort before its task.
    let upper = format!("{}A", "0".repeat(31));
    assert_eq!(code(json!({"pageToken": upper})).await?, -32_602);
    let lower = format!("{}a", "0".repeat(31));
    assert_eq!(code(json!({"pageToken": lower})).await?, -32_603);
    Ok(())
}

/// Lists the tasks of a target whose only page lists `tasks`, a page of one at a time,
/// following each token; answers the IDs listed, or the error.
#[cfg(unix)]
async fn follow_tokens(tasks: &[u8]) -> crate::test_daemon::TestResult<Vec<String>> {
    use ma2a_core::service::{AgentAccept, AgentReply, AgentTaskId};

    use crate::test_daemon::{Daemon, answering, summary};
    let page = tasks
        .iter()
        .map(|byte| summary(AgentTaskId::from_bytes([*byte; 16]), "worker"))
        .collect::<Result<_, _>>()?;
    let script = answering(AgentAccept::Admitted, &AgentReply::Page(page, None))?;
    let daemon = Daemon::start("list", script)?;
    let (backend, route) = Backend::standing_in(&daemon)?;
    let served = Served {
        backend: &backend,
        route: &route,
        base: "http://127.0.0.1",
    };
    let (mut listed, mut token) = (Vec::new(), String::new());
    loop {
        let params = json!({"pageSize": 1, "pageToken": token});
        let page = list(served, &params).await.map_err(|error| error.message)?;
        let text = |value: &Value, name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let ids = page
            .get("tasks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten();
        listed.extend(ids.map(|task| text(task, "id")));
        token = text(&page, "nextPageToken");
        if token.is_empty() {
            return Ok(listed);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn every_remote_task_is_reachable_by_its_tokens() -> Result<(), Box<dyn std::error::Error>> {
    let id = |byte: u8| format!("{byte:02x}").repeat(16);
    assert_eq!(follow_tokens(&[1, 2]).await.ok(), Some(vec![id(1), id(2)]));
    // A target listing out of order is not believed, rather than leaving a task behind.
    assert!(follow_tokens(&[2, 1]).await.is_err());
    assert!(follow_tokens(&[1, 1]).await.is_err());
    Ok(())
}

#[cfg(unix)]
mod filter_consistency {
    use ma2a_core::service::{
        AgentAccept, AgentMessage, AgentReply, AgentTaskDetails, AgentTaskId, AgentTaskState,
        FrameKind, MessagePart, MessageRole, ServiceError,
    };
    use serde_json::{Value, json};

    use super::{Backend, Served, list};
    use crate::test_daemon::{Daemon, Script, TestResult, answering, summary};

    fn task_id(byte: u8) -> AgentTaskId {
        AgentTaskId::from_bytes([byte; 16])
    }

    fn detail_script(byte: u8, state: AgentTaskState, changed_ms: u64) -> TestResult<Script> {
        let mut item = summary(task_id(byte), "worker")?;
        (item.state, item.changed_ms) = (state, changed_ms);
        let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".into())])?;
        let details = AgentTaskDetails::new(item, input);
        answering(
            AgentAccept::Admitted,
            &AgentReply::Details(Box::new(details)),
        )
    }

    fn waiting_page() -> TestResult<Script> {
        let page = [1_u8, 2, 3]
            .into_iter()
            .map(|byte| {
                let mut item = summary(task_id(byte), "worker")?;
                (item.state, item.changed_ms) = (AgentTaskState::Waiting, 2_000);
                Ok(item)
            })
            .collect::<TestResult<Vec<_>>>()?;
        answering(AgentAccept::Admitted, &AgentReply::Page(page, None))
    }

    async fn run(scripts: Vec<Script>, params: Value) -> TestResult<Value> {
        let daemon = Daemon::paging("list-filter", scripts)?;
        let (backend, route) = Backend::standing_in(&daemon)?;
        let served = Served {
            backend: &backend,
            route: &route,
            base: "http://127.0.0.1",
        };
        list(served, &params)
            .await
            .map_err(|error| Box::<dyn std::error::Error + Send + Sync>::from(error.message))
    }

    fn shown(page: &Value) -> Vec<(String, String)> {
        page.get("tasks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|task| {
                Some((
                    task.get("id")?.as_str()?.to_owned(),
                    task.pointer("/status/state")?.as_str()?.to_owned(),
                ))
            })
            .collect()
    }

    #[tokio::test]
    async fn a_task_that_leaves_the_filter_before_its_details_are_read_is_omitted() -> TestResult {
        let waiting = |byte: u8| {
            (
                task_id(byte).to_string(),
                "TASK_STATE_INPUT_REQUIRED".to_owned(),
            )
        };
        // With artifacts or history: task 1 resumes before its details read, so it is omitted
        // and tasks 2 and 3 fill the page of 2 with consistent totalSize and no nextPageToken.
        for params in [
            json!({"status": "TASK_STATE_INPUT_REQUIRED", "includeArtifacts": true, "pageSize": 2}),
            json!({"status": "TASK_STATE_INPUT_REQUIRED", "historyLength": 2, "pageSize": 2}),
        ] {
            let scripts = vec![
                waiting_page()?,
                detail_script(1, AgentTaskState::Running, 3_000)?,
                detail_script(2, AgentTaskState::Waiting, 2_000)?,
                detail_script(3, AgentTaskState::Waiting, 2_000)?,
            ];
            let page = run(scripts, params).await?;
            assert_eq!(shown(&page), [waiting(2), waiting(3)]);
            assert_eq!(page.get("totalSize"), Some(&json!(2)));
            assert_eq!(page.get("nextPageToken"), Some(&json!("")));
        }

        // With pageSize 1: task 1 drops out, task 2 fills the page, and task 3 stays next.
        let scripts = vec![
            waiting_page()?,
            detail_script(1, AgentTaskState::Running, 3_000)?,
            detail_script(2, AgentTaskState::Waiting, 2_000)?,
        ];
        let page = run(
            scripts,
            json!({"status": "TASK_STATE_INPUT_REQUIRED", "includeArtifacts": true, "pageSize": 1}),
        )
        .await?;
        assert_eq!(shown(&page), [waiting(2)]);
        assert_eq!(page.get("totalSize"), Some(&json!(2)));
        assert_eq!(
            page.get("nextPageToken"),
            Some(&json!(task_id(2).to_string()))
        );

        // Task 1 removed by retention before its details read is also skipped cleanly.
        let missing = vec![(FrameKind::Reject, vec![ServiceError::UnknownTask.code()])];
        let scripts = vec![
            waiting_page()?,
            missing,
            detail_script(2, AgentTaskState::Waiting, 2_000)?,
            detail_script(3, AgentTaskState::Waiting, 2_000)?,
        ];
        let page = run(
            scripts,
            json!({"status": "TASK_STATE_INPUT_REQUIRED", "includeArtifacts": true, "pageSize": 2}),
        )
        .await?;
        assert_eq!(shown(&page), [waiting(2), waiting(3)]);
        assert_eq!(page.get("totalSize"), Some(&json!(2)));
        assert_eq!(page.get("nextPageToken"), Some(&json!("")));

        // Bare page renders directly from the list summaries without reading details.
        let page = run(
            vec![waiting_page()?],
            json!({"status": "TASK_STATE_INPUT_REQUIRED", "pageSize": 2}),
        )
        .await?;
        assert_eq!(shown(&page), [waiting(1), waiting(2)]);
        assert_eq!(page.get("totalSize"), Some(&json!(3)));
        assert_eq!(
            page.get("nextPageToken"),
            Some(&json!(task_id(2).to_string()))
        );
        Ok(())
    }
}
