//! How a supervisor's report ends the link of an instance.

use ma2a_core::service::Termination;
use ma2a_services::supervisor::{EndCause, SupervisorStatus};

use super::link::{Linked, concluded};

#[test]
fn only_a_report_that_the_process_ended_ends_the_instance() {
    let ended = SupervisorStatus::Ended(Termination::Exited(0), EndCause::Exited, 3);
    assert!(matches!(concluded(&ended.to_bytes()), Linked::Ended));

    // The supervisor also answers a detach it decided on itself, for example after a write
    // the instance left waiting; the link then connects again.
    let running = SupervisorStatus::Running(1, 2);
    assert!(matches!(concluded(&running.to_bytes()), Linked::Broken));
    assert!(matches!(concluded(b"not a status"), Linked::Broken));
}
