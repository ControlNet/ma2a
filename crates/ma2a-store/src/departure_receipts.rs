//! Which generation removed the sender of one departure request, for an authority to answer a
//! retry of that request the same way.

use ma2a_core::{EndpointId, RequestId, SpaceId};
use rusqlite::{OptionalExtension as _, Transaction};

use crate::{Repository, StoreError};

/// One departure request an owner-signed removal answers: who asked, and with which request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepartureRequest {
    pub(crate) endpoint_id: EndpointId,
    pub(crate) request_id: RequestId,
}

impl DepartureRequest {
    /// Names the departure request `request_id` sent by `endpoint_id`.
    pub const fn new(endpoint_id: EndpointId, request_id: RequestId) -> Self {
        Self {
            endpoint_id,
            request_id,
        }
    }
}

/// Records that `generation` of `space_id` answered `request`, in the transaction that commits it.
pub(crate) fn record(
    transaction: &Transaction<'_>,
    (space_id, generation): (SpaceId, u64),
    request: DepartureRequest,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO departure_receipts(space_id, endpoint_id, request_id, generation)
         VALUES (?1, ?2, ?3, ?4)",
        (
            space_id.as_bytes().as_slice(),
            request.endpoint_id.as_bytes().as_slice(),
            request.request_id.as_bytes().as_slice(),
            i64::try_from(generation).map_err(|_| StoreError::SchemaMismatch {
                detail: "departure generation exceeds SQLite integer range",
            })?,
        ),
    )?;
    Ok(())
}

impl Repository {
    /// Returns the generation of `space_id` that answered `request` with a removal, if any.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row cannot be read.
    pub fn departure_receipt(
        &self,
        space_id: SpaceId,
        request: DepartureRequest,
    ) -> Result<Option<u64>, StoreError> {
        let generation = self
            .connection
            .query_row(
                "SELECT generation FROM departure_receipts
                 WHERE space_id = ?1 AND endpoint_id = ?2 AND request_id = ?3",
                (
                    space_id.as_bytes().as_slice(),
                    request.endpoint_id.as_bytes().as_slice(),
                    request.request_id.as_bytes().as_slice(),
                ),
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        generation
            .map(|generation| {
                u64::try_from(generation).map_err(|_| StoreError::SchemaMismatch {
                    detail: "negative departure generation",
                })
            })
            .transpose()
    }
}
