//! Fairness between targeted and global control rounds in the round queue.

use std::error::Error;

use ma2a_net::EndpointSecret;
use tokio::sync::oneshot;

use super::ControlRoundQueue;
use crate::control_sync::ControlRoundScope;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[test]
fn a_global_round_is_not_starved_by_a_targeted_request_during_every_round() -> TestResult {
    // Given: a targeted round running, and a global waiter behind it.
    let peer = EndpointSecret::parse(&[0x42; 32])?.endpoint_id();
    let mut queue = ControlRoundQueue::default();
    let mut active = queue
        .request(ControlRoundScope::peer(peer), None)
        .ok_or("active round missing")?;
    let (global_reply, mut global_response) = oneshot::channel();
    assert!(
        queue
            .request(ControlRoundScope::all(), Some(global_reply))
            .is_none()
    );

    // When: another targeted request arrives during every round.
    let mut global_ran = false;
    for _ in 0..3 {
        assert!(queue.request(ControlRoundScope::peer(peer), None).is_none());
        for waiter in queue.complete(active.id(), true) {
            waiter.send(Ok(1));
        }
        active = queue.take_pending().ok_or("next round missing")?;
        global_ran |= active.scope() == &ControlRoundScope::all();
    }
    for waiter in queue.complete(active.id(), true) {
        waiter.send(Ok(1));
    }

    // Then
    assert!(global_ran);
    assert_eq!(global_response.try_recv()??, 1);
    Ok(())
}
