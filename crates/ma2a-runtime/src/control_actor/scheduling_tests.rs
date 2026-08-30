use std::error::Error;

use ma2a_net::EndpointSecret;
use tokio::sync::oneshot;

use super::ControlRoundQueue;
use crate::control_sync::{ControlRoundRequest, ControlRoundScope, ControlRoundTrigger};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[test]
fn targeted_scope_retains_the_requested_eligible_peer() -> TestResult {
    // Given
    let local = EndpointSecret::parse(&[0x10; 32])?.endpoint_id();
    let requested = EndpointSecret::parse(&[0x20; 32])?.endpoint_id();
    let other = EndpointSecret::parse(&[0x30; 32])?.endpoint_id();

    // When
    let selected = ControlRoundRequest {
        local_endpoint_id: local,
        rotation: 7,
        now_ms: 0,
        scope: ControlRoundScope::peer(requested),
    }
    .select(&[other, requested]);

    // Then
    assert_eq!(selected, vec![requested]);
    Ok(())
}

#[test]
fn older_round_does_not_complete_a_newer_targeted_waiter() -> TestResult {
    // Given
    let peer = EndpointSecret::parse(&[0x40; 32])?.endpoint_id();
    let mut queue = ControlRoundQueue::default();
    let (older_reply, mut older_response) = oneshot::channel();
    let older = queue
        .request(ControlRoundScope::all(), Some(older_reply))
        .ok_or("older round missing")?;
    let (newer_reply, mut newer_response) = oneshot::channel();
    let pending = queue.request(ControlRoundScope::peer(peer), Some(newer_reply));

    // When
    let completed = queue.complete(older.id());

    // Then
    assert!(pending.is_none());
    assert_eq!(completed.len(), 1);
    for reply in completed {
        let _unsent = reply.send(Ok(1));
    }
    assert_eq!(older_response.try_recv()??, 1);
    assert!(matches!(
        newer_response.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    let newer = queue.take_pending().ok_or("pending round missing")?;
    assert_eq!(newer.scope(), &ControlRoundScope::peer(peer));
    assert_eq!(queue.complete(newer.id()).len(), 1);
    Ok(())
}

#[test]
fn every_control_trigger_maps_to_the_expected_round_scope() -> TestResult {
    // Given
    let peer = EndpointSecret::parse(&[0x50; 32])?.endpoint_id();
    let global = [
        ControlRoundTrigger::Startup,
        ControlRoundTrigger::ManifestAdvanced,
        ControlRoundTrigger::AddressAdvanced,
        ControlRoundTrigger::RelayAdvanced,
        ControlRoundTrigger::EnrollmentCompleted,
        ControlRoundTrigger::Periodic,
    ];

    // When
    let scopes = global.map(ControlRoundTrigger::scope);
    let explicit = ControlRoundTrigger::Explicit(ControlRoundScope::peer(peer)).scope();

    // Then
    assert!(
        scopes
            .into_iter()
            .all(|scope| scope == ControlRoundScope::all())
    );
    assert_eq!(explicit, ControlRoundScope::peer(peer));
    Ok(())
}
