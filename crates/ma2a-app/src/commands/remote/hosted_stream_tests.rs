//! Exit statuses and log cursors of `process logs` and `attach`.

use ma2a_core::service::{EndReason, Termination};

use super::{MAX_RETAINED_READS, another, effective_reason, exit_code};

#[test]
fn interrupted_delivery_turns_completed_attachment_into_detach() {
    assert_eq!(
        effective_reason(EndReason::Completed, true),
        EndReason::Cancelled
    );
    assert_eq!(
        effective_reason(EndReason::Completed, false),
        EndReason::Completed
    );
    assert_eq!(
        exit_code(
            effective_reason(EndReason::Completed, true),
            Some(Termination::Exited(42)),
        ),
        0
    );
}

#[test]
fn an_attachment_exits_with_the_status_of_the_process_it_saw_end() {
    assert_eq!(
        exit_code(EndReason::Completed, Some(Termination::Exited(3))),
        3
    );
    assert_eq!(
        exit_code(EndReason::Completed, Some(Termination::Signaled(9))),
        137
    );
    // Ended without an exit status: it never started, was lost, or its end is unknown.
    assert_eq!(exit_code(EndReason::Completed, None), 255);
    assert_eq!(
        exit_code(EndReason::Completed, Some(Termination::Unknown)),
        255
    );
    // Detached: the Process may still run.
    assert_eq!(exit_code(EndReason::Cancelled, None), 0);
    assert_eq!(
        exit_code(EndReason::Cancelled, Some(Termination::Exited(3))),
        0
    );
}

#[test]
fn a_log_cursor_that_goes_back_is_refused() {
    assert!(another((1, None), (0, false)).is_ok_and(|another| another));
    assert!(another((2, Some(10)), (20, false)).is_ok_and(|another| another));
    assert!(another((2, Some(10)), (10, false)).is_ok_and(|another| !another));
    assert!(another((2, Some(10)), (9, false)).is_err());
}

#[test]
fn a_final_reply_whose_cursor_goes_back_is_refused() {
    // A read from cursor 100 that ends the Process may not resume anyone at 90.
    assert!(another((1, Some(100)), (90, true)).is_err());
    assert!(another((1, Some(100)), (100, true)).is_ok_and(|another| !another));
    assert!(another((1, None), (0, true)).is_ok_and(|another| !another));
}

#[test]
fn a_log_cursor_still_creeping_after_the_last_read_is_refused() {
    let last = MAX_RETAINED_READS;
    assert!(another((last - 1, Some(10)), (11, false)).is_ok_and(|another| another));
    // A last read that ends where the one before it did is answered as it is, and so is
    // one that needs no further read.
    assert!(another((last, Some(10)), (10, false)).is_ok_and(|another| !another));
    assert!(another((last, Some(10)), (11, true)).is_ok_and(|another| !another));
    assert!(another((last, Some(10)), (11, false)).is_err());
}
