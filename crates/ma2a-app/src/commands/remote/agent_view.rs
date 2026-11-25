//! JSON views of remote Agent answers, shaped like the local API's `agent_*` results.

use ma2a_core::service::{AgentCard, AgentId, AgentReply};
use ma2a_runtime::api::{agent_task_details_value, agent_task_value};
use serde_json::{Value, json};

fn card(card: &AgentCard) -> Value {
    json!({
        "agent": card.id.as_str(),
        "description": card.description,
        "available": card.available,
        "max_tasks": card.max_tasks,
    })
}

pub(super) fn view(reply: &AgentReply) -> Value {
    match reply {
        AgentReply::Cards(cards, next) => json!({
            "agents": cards.iter().map(card).collect::<Vec<_>>(),
            "next": next.as_ref().map(AgentId::as_str),
        }),
        AgentReply::Summary(summary) => agent_task_value(summary),
        AgentReply::Details(details) => agent_task_details_value(details),
        AgentReply::Page(tasks, next) => json!({
            "tasks": tasks.iter().map(agent_task_value).collect::<Vec<_>>(),
            "next": next.map(|id| id.to_string()),
        }),
    }
}
