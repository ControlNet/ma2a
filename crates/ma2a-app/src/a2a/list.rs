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
        let summary = task.get("task").unwrap_or(task);
        let changed_ms = summary
            .get("changed_at_ms")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let state = summary
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.context
            .is_none_or(|context| task_id(summary) == context)
            && self
                .status
                .is_none_or(|status| mapping::state(state) == status)
            && self
                .changed_after_ms
                .is_none_or(|after| changed_ms >= after)
    }
}

/// Whether a task disappeared before its details or events could be read.
fn vanished(refusal: &Refusal) -> bool {
    matches!(refusal.0.as_str(), "not_found" | "unknown_task")
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
    let needs_details = detail.artifacts || detail.history.is_some_and(|history| history > 0);
    let mut tasks = Vec::with_capacity(page_size.min(matching.len().saturating_sub(start)));
    let mut last_id = None;
    let mut dropped = 0_usize;
    let mut visited = start;
    for summary in matching.iter().skip(start) {
        if tasks.len() >= page_size {
            break;
        }
        visited += 1;
        let id = task_id(summary);
        // History and artifacts need each task's details; recheck the filter against the
        // fetched details so no returned entry fails the filter on its displayed fields.
        let task = if needs_details {
            let details = match served.backend.details(served.route, &id).await {
                Ok(details) => details,
                Err(refusal) if vanished(&refusal) => {
                    dropped += 1;
                    continue;
                }
                Err(refusal) => return Err(refused(refusal)),
            };
            if !filter.keeps(&details) {
                dropped += 1;
                continue;
            }
            match view::of(served, &details, detail).await {
                Ok(task) => task,
                Err(refusal) if vanished(&refusal) => {
                    dropped += 1;
                    continue;
                }
                Err(refusal) => return Err(refused(refusal)),
            }
        } else {
            let bare = Detail {
                history: Some(0),
                artifacts: false,
            };
            mapping::task(summary, (&[], served.base), bare)
        };
        last_id = Some(id);
        tasks.push(task);
    }
    let next = if visited < matching.len() {
        last_id
    } else {
        None
    };
    Ok(json!({
        "tasks": tasks,
        "nextPageToken": next.unwrap_or_default(),
        "pageSize": page_size,
        "totalSize": matching.len() - dropped,
    }))
}

#[cfg(all(test, unix))]
#[path = "list_local_tests.rs"]
mod local_tests;
#[cfg(test)]
#[path = "list_tests.rs"]
mod tests;
