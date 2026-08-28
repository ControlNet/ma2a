//! Checked-in machine-consumable golden contract for local API v1.

/// Exact ordered result inventory.
pub const RESULT_NAMES: [&str; 21] = [
    "handshake",
    "status",
    "endpoint_info",
    "space_created",
    "spaces",
    "space",
    "space_invitation_created",
    "space_redeemed",
    "space_revoked",
    "control_sync_status",
    "control_sync_triggered",
    "private_relay_configured",
    "private_relay_status",
    "public_relay_configured",
    "public_relay_status",
    "echo",
    "ui_password_set",
    "ui_password_reset",
    "sessions_revoked",
    "snapshot",
    "shutting_down",
];

/// Exact ordered Todo 2 error inventory used by the local API.
pub const ERROR_NAMES: [&str; 9] = [
    "version_mismatch",
    "invalid_input",
    "unauthorized",
    "not_found",
    "conflict",
    "expired",
    "rollback",
    "unavailable",
    "internal",
];

/// Canonical JSON golden consumed by Rust tests and generated TypeScript checks.
pub const LOCAL_API_SCHEMA_JSON: &str = r#"{"schema":"ma2a.local-api","version":1,"compatibility":"exact","bounds":{"request_bytes":16384,"response_bytes":65536,"event_bytes":16384,"text_bytes":4096,"collection_items":256},"ids":{"endpoint_id":"validated-lowercase-hex-32-bytes","space_id":"lowercase-hex-32-bytes","request_id":"lowercase-hex-16-bytes"},"commands":["handshake","status","endpoint_info","space_create","space_list","space_show","space_invite","space_redeem","space_revoke","control_sync_status","control_sync_trigger","private_relay_configure","private_relay_status","public_relay_configure","public_relay_status","echo_call","ui_password_set","ui_password_reset","session_revoke_all","snapshot_fetch","graceful_shutdown"],"results":["handshake","status","endpoint_info","space_created","spaces","space","space_invitation_created","space_redeemed","space_revoked","control_sync_status","control_sync_triggered","private_relay_configured","private_relay_status","public_relay_configured","public_relay_status","echo","ui_password_set","ui_password_reset","sessions_revoked","snapshot","shutting_down"],"errors":["version_mismatch","invalid_input","unauthorized","not_found","conflict","expired","rollback","unavailable","internal"],"events":["snapshot_invalidated","endpoint_changed","spaces_changed","control_sync_changed","relay_candidates_changed","relay_state_changed","reachability_changed","echo_summary_changed","ui_auth_changed"],"snapshot_fields":["revision","endpoint","spaces","control_sync","relay_candidates","observed_relay_state","reachability","recent_echo_summary","ui_auth"],"handshake_fields":["runtime_version","endpoint_id","revision","initialized","password_set","capabilities"],"request_id":{"same_payload":"replay","different_payload":"conflict","persistence":"out-of-scope"},"events_policy":{"delivery":"best-effort","durable_replay":false,"gap":"resnapshot","disconnect":"resnapshot"}}"#;

/// Independently pinned SHA-256 of `LOCAL_API_SCHEMA_JSON`.
pub const LOCAL_API_SCHEMA_SHA256: &str =
    "6703648a604f92cf5011b449ca42d4242fa9c6453b5ecf3a94c98ef98009dcf6";
