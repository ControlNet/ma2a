use ma2a_core::{ControlCursorEntryV1, EndpointId};

/// Returns whether a receiver with `cursors` has not yet accepted `sequence` of one subject.
///
/// A subject without a cursor entry is one the receiver has never accepted anything from,
/// so even its first artifact, at sequence zero, is news to it.
pub fn is_unseen(
    cursors: &[ControlCursorEntryV1],
    (endpoint_id, sequence): (EndpointId, u64),
) -> bool {
    cursors
        .binary_search_by_key(&endpoint_id, |cursor| cursor.endpoint_id())
        .ok()
        .and_then(|index| cursors.get(index))
        .is_none_or(|cursor| sequence > cursor.sequence())
}

#[cfg(test)]
mod tests {
    use ma2a_core::{ControlCursorEntryV1, EndpointId};

    use super::is_unseen;
    use crate::{EndpointSecret, InvalidEndpointSecret};

    fn endpoint(byte: u8) -> Result<EndpointId, InvalidEndpointSecret> {
        EndpointSecret::parse(&[byte; 32]).map(|secret| secret.endpoint_id())
    }

    #[test]
    fn a_first_artifact_is_unseen_by_a_receiver_without_its_cursor()
    -> Result<(), InvalidEndpointSecret> {
        assert!(is_unseen(&[], (endpoint(1)?, 0)));
        let cursors = [ControlCursorEntryV1::new(endpoint(2)?, 0)];
        assert!(is_unseen(&cursors, (endpoint(1)?, 0)));
        Ok(())
    }

    #[test]
    fn only_a_later_sequence_is_unseen_by_a_receiver_with_its_cursor()
    -> Result<(), InvalidEndpointSecret> {
        let cursors = [ControlCursorEntryV1::new(endpoint(1)?, 3)];
        assert!(!is_unseen(&cursors, (endpoint(1)?, 0)));
        assert!(!is_unseen(&cursors, (endpoint(1)?, 3)));
        assert!(is_unseen(&cursors, (endpoint(1)?, 4)));
        Ok(())
    }
}
