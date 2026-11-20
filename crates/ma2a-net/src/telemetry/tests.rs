use ma2a_core::EndpointId;

use super::{ConnectionTelemetry, MAX_REMOTE_ENDPOINTS};
use crate::{EndpointSecret, InvalidEndpointSecret};

#[test]
fn a_full_cache_forgets_the_remote_observed_longest_ago() -> Result<(), InvalidEndpointSecret> {
    // Given: a full cache whose smallest Endpoint ID was observed last, at the same clock time.
    let mut remotes = (1..=u8::try_from(MAX_REMOTE_ENDPOINTS + 1).unwrap_or(u8::MAX))
        .map(|byte| EndpointSecret::parse(&[byte; 32]).map(|secret| secret.endpoint_id()))
        .collect::<Result<Vec<EndpointId>, _>>()?;
    remotes.sort_unstable();
    let newcomer = remotes.pop();
    let telemetry = ConnectionTelemetry::default();
    for remote in &remotes {
        telemetry.record_connecting(*remote, 0);
    }
    let (smallest, observed_longest_ago) = (remotes.first(), remotes.get(1));
    if let Some(smallest) = smallest {
        telemetry.record_connecting(*smallest, 0);
    }

    // When: one more remote is observed.
    if let Some(newcomer) = newcomer {
        telemetry.record_connecting(newcomer, 0);
    }

    // Then: the remote observed longest ago is gone; the recent one and the newcomer stay.
    let held = |remote: Option<&EndpointId>| {
        remote.is_some_and(|remote| !telemetry.observations(*remote).is_empty())
    };
    assert!(held(smallest));
    assert!(!held(observed_longest_ago));
    assert!(held(newcomer.as_ref()));
    assert_eq!(telemetry.latest_observations().len(), MAX_REMOTE_ENDPOINTS);
    Ok(())
}
