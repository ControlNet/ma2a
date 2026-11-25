//! How local API refusals become adapter refusals.

use ma2a_runtime::api::{AGENT_UNAVAILABLE_REMEDIATION, TASK_ENDED_REMEDIATION};
use serde_json::json;

use super::{Refusal, refused};

#[test]
fn a_message_to_an_ended_task_is_told_apart_from_a_reused_request() {
    let error =
        |remediation| json!({"version": 1, "error": "conflict", "remediation": remediation});
    assert_eq!(
        refused(&error(json!(TASK_ENDED_REMEDIATION))),
        Some(Refusal::named("task_ended"))
    );
    assert_eq!(
        refused(&error(json!(null))),
        Some(Refusal::named("conflict"))
    );
    // A stopped Agent answers `conflict` too, told apart the same way.
    assert_eq!(
        refused(&error(json!(AGENT_UNAVAILABLE_REMEDIATION))),
        Some(Refusal::named("agent_unavailable"))
    );
    assert_eq!(refused(&json!({"version": 1, "result": {}})), None);
}
