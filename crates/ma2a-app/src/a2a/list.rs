//! `ListTasks`: every retained task of the route's Agent this caller may see, filtered,
//! then paged by task ID.

use serde_json::{Value, json};

use super::{
    backend::Refusal,
    mapping::{self, Detail},
    methods::{detail, task_id},
    rpc::{RpcError, count},
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

/// Every task of the route's Agent, across all pages.
async fn all(served: Served<'_>) -> Result<Vec<Value>, Refusal> {
    let (mut all, mut after) = (Vec::new(), None::<String>);
    loop {
        let (tasks, next) = served.backend.tasks(served.route, after.as_deref()).await?;
        all.extend(tasks);
        after = next;
        if after.is_none() {
            return Ok(all);
        }
    }
}

pub(super) async fn list(served: Served<'_>, params: &Value) -> Result<Value, RpcError> {
    let refused = |refusal: Refusal| RpcError::refused("ListTasks", &refusal);
    let page_size = count(params, "pageSize")?.unwrap_or(DEFAULT_PAGE);
    if !(1..=MAX_PAGE).contains(&page_size) {
        return Err(RpcError::invalid_params("`pageSize` is 1 to 100"));
    }
    let wanted = |key: &str| {
        params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let changed_after_ms = match wanted("statusTimestampAfter") {
        Some(at) => Some(time::parse(at).ok_or_else(|| {
            RpcError::invalid_params("`statusTimestampAfter` is an ISO 8601 UTC timestamp")
        })?),
        None => None,
    };
    let filter = Filter {
        context: wanted("contextId"),
        status: wanted("status"),
        changed_after_ms,
    };
    let all = all(served).await.map_err(refused)?;
    let matching: Vec<&Value> = all.iter().filter(|task| filter.keeps(task)).collect();
    let start = match wanted("pageToken") {
        Some(token) => matching
            .iter()
            .position(|task| task_id(task) == token)
            .map(|index| index + 1)
            .ok_or_else(|| RpcError::invalid_params("`pageToken` is not from this listing"))?,
        None => 0,
    };
    let page: Vec<&Value> = matching
        .iter()
        .skip(start)
        .take(page_size)
        .copied()
        .collect();
    let more = start + page.len() < matching.len();
    let artifacts = params.get("includeArtifacts").and_then(Value::as_bool) == Some(true);
    let detail = detail(params, artifacts)?;
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
