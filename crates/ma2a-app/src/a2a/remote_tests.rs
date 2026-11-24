//! Remote task paging, artifact references, and the deadline of one remote operation.

use ma2a_core::service::{AgentTaskId, ServiceError};

use super::{CALL_DEADLINE, Refusal, bounded, matches_reference, moves_past};

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

#[tokio::test(start_paused = true)]
async fn a_target_that_never_answers_fails_as_timed_out() {
    // Given: a target that never answers.
    let call = bounded(CALL_DEADLINE, std::future::pending::<Result<(), Refusal>>());

    // When
    let answered = tokio::time::timeout(CALL_DEADLINE * 2, call).await;

    // Then
    assert_eq!(
        answered,
        Ok(Err(Refusal::named(ServiceError::TimedOut.name())))
    );
}
