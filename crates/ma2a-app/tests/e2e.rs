//! Focused Phase One E2E test target required by the release gate.

#[path = "e2e/control_sync.rs"]
mod control_sync;
#[path = "control_sync_e2e/adversarial.rs"]
mod control_sync_e2e_adversarial;
#[path = "control_sync_e2e/artifacts.rs"]
mod control_sync_e2e_artifacts;
#[path = "control_sync_e2e/support.rs"]
mod control_sync_e2e_support;
#[path = "daemon_lifecycle.rs"]
mod daemon_lifecycle;
#[path = "e2e/harness.rs"]
mod harness;
#[path = "e2e/ipc.rs"]
mod ipc;
#[path = "e2e/recovery.rs"]
mod recovery;
#[path = "e2e/relay_common.rs"]
mod relay_common;
#[path = "e2e/relay_lifecycle.rs"]
mod relay_lifecycle;
#[path = "e2e/relay_matrix.rs"]
mod relay_matrix;
#[path = "e2e/relay_scenarios.rs"]
mod relay_scenarios;
#[path = "e2e/relay_sync.rs"]
mod relay_sync;
#[path = "e2e/relay_target.rs"]
mod relay_target;

#[path = "zero_space.rs"]
mod authorization;
#[path = "zero_space_isolation.rs"]
mod authorization_isolation;
#[path = "echo_e2e.rs"]
mod echo;
#[path = "enrollment.rs"]
mod enrollment;
#[path = "enrollment_replay.rs"]
mod enrollment_replay;
#[path = "../../ma2a-net/tests/relay_tls_paths.rs"]
mod relay_tls;
#[path = "../../ma2a-runtime/tests/web_security.rs"]
mod web;
// CLIPPY-ALLOW: The release-gate binary composes two standalone Web integration targets.
#[allow(
    clippy::duplicate_mod,
    reason = "release gate composes standalone Web integration targets"
)]
#[path = "../../ma2a-runtime/tests/web_session_mutation.rs"]
mod web_session;

async fn raw_endpoint(
    secret: &iroh::SecretKey,
) -> Result<ma2a_net::RuntimeEndpoint, Box<dyn std::error::Error + Send + Sync>> {
    let (enrollment_calls, _receiver) = tokio::sync::mpsc::channel(1);
    Ok(ma2a_net::RuntimeEndpoint::bind(
        ma2a_net::EndpointSecret::parse(&secret.to_bytes())?,
        enrollment_calls,
        None,
    )
    .await?)
}
