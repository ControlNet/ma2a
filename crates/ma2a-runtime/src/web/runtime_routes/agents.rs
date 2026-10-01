//! The read-only Agents view: what each Agent is and does, and its recent tasks.

use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse as _, Response},
};
use serde_json::{Map, Value, json};

use super::{WebState, payload};

/// The fields of an Agent the browser may see: never its program, arguments, working
/// directory, or environment.
const AGENT_FIELDS: [&str; 8] = [
    "agent",
    "description",
    "tools",
    "max_tasks",
    "enabled",
    "generation",
    "live_tasks",
    "instance",
];

fn shown(agent: &Value) -> Value {
    let fields: Map<String, Value> = AGENT_FIELDS
        .iter()
        .filter_map(|&field| Some((field.to_owned(), agent.get(field)?.clone())))
        .collect();
    Value::Object(fields)
}

/// The first page of defined Agents and of tasks, content-free: never a message or an
/// artifact.
pub(in crate::web) async fn agents(State(state): State<WebState>, headers: HeaderMap) -> Response {
    let agents = json!({"version": 1, "operation": "agent_list", "after": null});
    let tasks = json!({"version": 1, "operation": "agent_task_list", "agent": null, "after": null});
    let read = async {
        let agents = payload(&state, &headers, (&agents, "agents")).await?;
        let tasks = payload(&state, &headers, (&tasks, "agent_tasks")).await?;
        Ok::<_, axum::http::StatusCode>((agents, tasks))
    };
    match read.await {
        Ok((agents, tasks)) => {
            let shown: Vec<Value> = agents
                .get("agents")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(shown)
                .collect();
            let more = |page: &Value| page.get("next").is_some_and(|next| !next.is_null());
            Json(json!({
                "agents": shown,
                "more_agents": more(&agents),
                "tasks": tasks.get("tasks").cloned().unwrap_or_else(|| json!([])),
                "more_tasks": more(&tasks),
            }))
            .into_response()
        }
        Err(status) => status.into_response(),
    }
}
