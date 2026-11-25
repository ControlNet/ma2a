//! Remote task paging, artifact references, and the deadline of one remote operation.

use ma2a_core::service::AgentTaskId;

use super::{matches_reference, moves_past};

const fn id(byte: u8) -> AgentTaskId {
    AgentTaskId::from_bytes([byte; 16])
}

/// Whether a page listing `listed` and naming `next` moves past `after`.
fn moves(after: Option<u8>, listed: &[u8], next: Option<u8>) -> bool {
    let listed = listed.iter().map(|byte| id(*byte));
    moves_past(after.map(id), (listed, next.map(id)))
}

#[test]
fn a_page_must_move_past_the_cursor_sent() {
    // The first page has nothing to move past.
    assert!(moves(None, &[1], Some(1)));
    assert!(moves(Some(1), &[2, 3], Some(3)));
    assert!(moves(Some(1), &[2], None));
    // The same cursor again, one behind it, or a task already listed.
    assert!(!moves(Some(3), &[4], Some(3)));
    assert!(!moves(Some(3), &[], Some(2)));
    assert!(!moves(Some(3), &[3], None));
}

#[test]
fn content_must_be_what_the_artifact_reference_names() {
    let hash = *blake3::hash(b"result").as_bytes();
    let hex = blake3::hash(b"result").to_hex();
    assert!(matches_reference((6, &hex), (6, &hash)));
    // Another size, another hash, or a reference that names no hash.
    assert!(!matches_reference((7, &hex), (6, &hash)));
    let other = blake3::hash(b"forged").to_hex();
    assert!(!matches_reference((6, &other), (6, &hash)));
    assert!(!matches_reference((6, ""), (6, &hash)));
}

#[cfg(unix)]
mod through_the_daemon {
    use ma2a_core::service::ServiceError;
    use tokio::time::Instant;

    use super::{super::CALL_DEADLINE, id};
    use crate::a2a::{
        backend::Refusal,
        remote_fixture::{Daemon, TestResult},
    };

    #[tokio::test(start_paused = true)]
    async fn a_target_that_never_answers_fails_as_timed_out() -> TestResult {
        // Given: a target whose stream opens and then stays silent.
        let daemon = Daemon::start("silent", Vec::new())?;
        let (task, started) = (id(1).to_string(), Instant::now());

        // When
        let got = super::super::details(&daemon.backend, &daemon.route, &task);
        let answered = tokio::time::timeout(CALL_DEADLINE * 2, got).await;

        // Then: the operation's own deadline ended it, not the transport's.
        let timed_out = Refusal::named(ServiceError::TimedOut.name());
        assert_eq!(answered.ok(), Some(Err(timed_out)));
        assert_eq!(started.elapsed(), CALL_DEADLINE);
        Ok(())
    }
}
