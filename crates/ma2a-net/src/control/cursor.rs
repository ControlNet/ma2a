use ma2a_core::{ControlCursorEntryV1, EndpointId};

/// Returns the accepted sequence for one subject, or zero when absent.
pub fn cursor_sequence(cursors: &[ControlCursorEntryV1], endpoint_id: EndpointId) -> u64 {
    cursors
        .binary_search_by_key(&endpoint_id, |cursor| cursor.endpoint_id())
        .ok()
        .and_then(|index| cursors.get(index))
        .map_or(0, |cursor| cursor.sequence())
}
