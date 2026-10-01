//! The read-only hosted-services view: what each service offers, its state, and its calls.

use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse as _, Response},
};
use serde_json::{Value, json};

use super::{WebState, payload};

/// One hosted service as the browser sees it. A view carries no call body, caller, or
/// result, so nothing is withheld beyond flattening its manifest.
fn shown(service: &Value) -> Value {
    let manifest = |field: &str| {
        service
            .pointer(&format!("/manifest/{field}"))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let field = |field: &str| service.get(field).cloned().unwrap_or(Value::Null);
    json!({
        "service": field("service"),
        "description": manifest("description"),
        "versions": manifest("versions"),
        "max_calls": manifest("max_calls"),
        "enabled": field("enabled"),
        "state": field("state"),
        "running_calls": field("running_calls"),
        "calls": field("calls"),
    })
}

/// The first page of this Runtime's hosted services.
pub(in crate::web) async fn hosted_services(
    State(state): State<WebState>,
    headers: HeaderMap,
) -> Response {
    let list = json!({"version": 1, "operation": "service_hosted_list", "after": null});
    match payload(&state, &headers, (&list, "hosted_services")).await {
        Ok(page) => {
            let services: Vec<Value> = page
                .get("services")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(shown)
                .collect();
            let more = page.get("next").is_some_and(|next| !next.is_null());
            Json(json!({"services": services, "more": more})).into_response()
        }
        Err(status) => status.into_response(),
    }
}
