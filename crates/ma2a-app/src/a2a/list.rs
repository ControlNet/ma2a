//! `ListTasks`: every retained task of the route's Agent this caller may see, filtered,
//! then paged by task ID.

use ma2a_core::service::AgentTaskId;
use serde_json::{Value, json};

use super::{
    backend::Refusal,
    mapping::{self, Detail},
    methods::{detail, task_id},
    rpc::{RpcError, count, flag, optional},
    time,
    view::{self, Served},
};

/// Default page.
const DEFAULT_PAGE: usize = 50;
/// Largest page.
const MAX_PAGE: usize = 100;

/// What a listing keeps.
struct Filter<'a> {
    context: Option<&'a str>,
    status: Option<&'a str>,
    changed_after_ms: Option<u64>,
}

impl Filter<'_> {
    fn keeps(&self, task: &Value) -> bool {
        let changed_ms = ["/ended_at_ms", "/started_at_ms", "/created_at_ms"]
            .iter()
            .find_map(|pointer| task.pointer(pointer).and_then(Value::as_u64))
            .unwrap_or_default();
        let state = task
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.context.is_none_or(|context| task_id(task) == context)
            && self
                .status
                .is_none_or(|status| mapping::state(state) == status)
            && self
                .changed_after_ms
                .is_none_or(|after| changed_ms >= after)
    }
}

/// Most task pages read: a Runtime retains at most 256 unfinished and 1,024 finished tasks,
/// 32 to a page, so a target paging further is not believed.
const MAX_TASK_PAGES: usize = (256 + 1_024) / ma2a_core::service::MAX_TASK_PAGE + 1;

/// Every task of the route's Agent, across all pages.
async fn all(served: Served<'_>) -> Result<Vec<Value>, Refusal> {
    let (mut all, mut after) = (Vec::new(), None::<String>);
    for _ in 0..MAX_TASK_PAGES {
        let (tasks, next) = served.backend.tasks(served.route, after.as_deref()).await?;
        all.extend(tasks);
        after = next;
        if after.is_none() {
            return Ok(all);
        }
    }
    Err(Refusal::named("internal"))
}

pub(super) async fn list(served: Served<'_>, params: &Value) -> Result<Value, RpcError> {
    let refused = |refusal: Refusal| RpcError::refused("ListTasks", &refusal);
    let page_size = count(params, "pageSize")?.unwrap_or(DEFAULT_PAGE);
    if !(1..=MAX_PAGE).contains(&page_size) {
        return Err(RpcError::invalid_params("`pageSize` is 1 to 100"));
    }
    let changed_after_ms = match optional(params, "statusTimestampAfter")? {
        Some(at) => Some(time::parse(at).ok_or_else(|| {
            RpcError::invalid_params("`statusTimestampAfter` is an ISO 8601 UTC timestamp")
        })?),
        None => None,
    };
    let filter = Filter {
        context: optional(params, "contextId")?,
        status: optional(params, "status")?,
        changed_after_ms,
    };
    let token = optional(params, "pageToken")?;
    // A token is a task ID as listed, in lowercase: any other spelling would sort apart
    // from its task.
    if token.is_some_and(|token| AgentTaskId::parse(token).is_err()) {
        return Err(RpcError::invalid_params(
            "`pageToken` is not from this listing",
        ));
    }
    let artifacts = flag(params, "includeArtifacts")?;
    // A listing includes history only when `historyLength` asks for it.
    let detail = Detail {
        history: Some(detail(params, artifacts)?.history.unwrap_or(0)),
        artifacts,
    };
    // Every parameter is checked before any task is read.
    let all = all(served).await.map_err(refused)?;
    let matching: Vec<&Value> = all.iter().filter(|task| filter.keeps(task)).collect();
    // Tasks are listed in task ID order, so a page continues after its last ID even when
    // that task no longer matches.
    let start = token.map_or(0, |token| {
        matching
            .iter()
            .position(|task| task_id(task).as_str() > token)
            .unwrap_or(matching.len())
    });
    let page: Vec<&Value> = matching
        .iter()
        .skip(start)
        .take(page_size)
        .copied()
        .collect();
    let more = start + page.len() < matching.len();
    let mut tasks = Vec::with_capacity(page.len());
    for summary in &page {
        // History and artifacts need each task's details; a bare page needs none.
        let task = if detail.artifacts || detail.history.is_some_and(|history| history > 0) {
            view::task(served, &task_id(summary), detail)
                .await
                .map_err(refused)?
        } else {
            let bare = Detail {
                history: Some(0),
                artifacts: false,
            };
            mapping::task(summary, (&[], served.base), bare)
        };
        tasks.push(task);
    }
    let next = if more {
        page.last().map(|task| task_id(task))
    } else {
        None
    };
    Ok(json!({
        "tasks": tasks,
        "nextPageToken": next.unwrap_or_default(),
        "pageSize": page_size,
        "totalSize": matching.len(),
    }))
}

#[cfg(test)]
mod tests {
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
    async fn follow_tokens(tasks: &[u8]) -> crate::a2a::remote_fixture::TestResult<Vec<String>> {
        use ma2a_core::service::{AgentAccept, AgentReply, AgentTaskId};

        use crate::a2a::remote_fixture::{Daemon, answering, summary};
        let page = tasks
            .iter()
            .map(|byte| summary(AgentTaskId::from_bytes([*byte; 16]), "worker"))
            .collect::<Result<_, _>>()?;
        let script = answering(AgentAccept::Admitted, &AgentReply::Page(page, None))?;
        let daemon = Daemon::start("list", script)?;
        let served = Served {
            backend: &daemon.backend,
            route: &daemon.route,
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
    async fn every_remote_task_is_reachable_by_its_tokens() -> Result<(), Box<dyn std::error::Error>>
    {
        let id = |byte: u8| format!("{byte:02x}").repeat(16);
        assert_eq!(follow_tokens(&[1, 2]).await.ok(), Some(vec![id(1), id(2)]));
        // A target listing out of order is not believed, rather than leaving a task behind.
        assert!(follow_tokens(&[2, 1]).await.is_err());
        assert!(follow_tokens(&[1, 1]).await.is_err());
        Ok(())
    }
}
