use rusqlite::OptionalExtension as _;

use crate::StoreError;

pub(crate) fn highest_relay_sequence(
    transaction: &rusqlite::Transaction<'_>,
    space_id: &[u8; 32],
    endpoint_id: &[u8; 32],
) -> Result<Option<u64>, StoreError> {
    Ok(transaction
        .query_row(
            "SELECT sequence FROM relay_advertisement_state
             WHERE space_id = ?1 AND relay_endpoint_id = ?2",
            (space_id.as_slice(), endpoint_id.as_slice()),
            |row| row.get(0),
        )
        .optional()?)
}
