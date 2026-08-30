//! Focused Phase One E2E test target required by the release gate.

#[path = "e2e/control_sync.rs"]
mod control_sync;
#[path = "control_sync_e2e/adversarial.rs"]
mod control_sync_e2e_adversarial;
#[path = "control_sync_e2e/artifacts.rs"]
mod control_sync_e2e_artifacts;
#[path = "control_sync_e2e/support.rs"]
mod control_sync_e2e_support;
#[path = "e2e/harness.rs"]
mod harness;
#[path = "e2e/relay_scenarios.rs"]
mod relay_scenarios;
#[path = "e2e/relay_target.rs"]
mod relay_target;

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
