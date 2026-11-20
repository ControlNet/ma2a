//! Which generation removed the sender of one departure request, for an authority to answer a
//! retry of that request the same way.

use ma2a_core::{EndpointId, RequestId, SpaceChain, SpaceId};
use rusqlite::{OptionalExtension as _, Transaction};

use crate::{Repository, StoreError};

/// How many departure requests of one removed member may be bound to the generation that
/// removed it. Retries reuse their request ID, so a member that departs normally needs one.
pub const MAX_BOUND_DEPARTURES: i64 = 8;

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

/// Whether advancing `previous` to `chain` removes and revokes the sender of `request`, so that
/// the new generation may be recorded as its answer.
pub(crate) fn removes(
    previous: Option<&SpaceChain>,
    chain: &SpaceChain,
    request: DepartureRequest,
) -> bool {
    let member = |chain: &SpaceChain| {
        chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == request.endpoint_id)
    };
    previous.is_some_and(member)
        && !member(chain)
        && chain
            .revocations()
            .iter()
            .any(|revocation| revocation.endpoint_id() == request.endpoint_id)
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

    /// Records `generation` of `space_id`, already committed, as the answer to `request`.
    ///
    /// A departure that reaches the authority after its sender was already removed, by the
    /// owner or before receipts were kept, is answered with that earlier removal. Binding the
    /// request to it means a retry after the sender joined again gets the same answer instead
    /// of removing the new membership. The binding is refused unless that generation removed
    /// and revoked the sender; it changes no Space state, so the revision stays.
    ///
    /// # Errors
    /// Returns [`StoreError::SpaceNotFound`] without the Space, [`StoreError::SchemaMismatch`]
    /// when the generation does not remove the sender, [`StoreError::DepartureReceiptsFull`]
    /// once [`MAX_BOUND_DEPARTURES`] requests are bound to that removal, and [`StoreError`] when
    /// the row cannot be written, including when the request already has a receipt.
    pub fn bind_departure_receipt(
        &mut self,
        (space_id, generation): (SpaceId, u64),
        request: DepartureRequest,
    ) -> Result<(), StoreError> {
        let transaction = self.immediate()?;
        let chain = crate::space_rows::load_chain(&transaction, space_id)?
            .ok_or(StoreError::SpaceNotFound)?;
        let removed = cut_at(&chain, generation)
            .is_some_and(|(previous, cut)| removes(Some(&previous), &cut, request));
        if !removed {
            return Err(StoreError::SchemaMismatch {
                detail: "a departure receipt must name the member its generation removes",
            });
        }
        let bound = transaction.query_row(
            "SELECT count(*) FROM departure_receipts
             WHERE space_id = ?1 AND endpoint_id = ?2 AND generation = ?3",
            (
                space_id.as_bytes().as_slice(),
                request.endpoint_id.as_bytes().as_slice(),
                i64::try_from(generation).unwrap_or(i64::MAX),
            ),
            |row| row.get::<_, i64>(0),
        )?;
        if bound >= MAX_BOUND_DEPARTURES {
            return Err(StoreError::DepartureReceiptsFull);
        }
        record(&transaction, (space_id, generation), request)?;
        transaction.commit()?;
        Ok(())
    }
}

/// Returns `chain` just before and at `generation`, or `None` when it holds no such manifest.
/// Only the prefix just before `generation` is copied.
fn cut_at(chain: &SpaceChain, generation: u64) -> Option<(SpaceChain, SpaceChain)> {
    let mut prefix = SpaceChain::from_genesis(chain.genesis().clone()).ok()?;
    for manifest in chain.manifests() {
        if prefix.latest_generation().checked_add(1) == Some(generation) {
            let previous = prefix.clone();
            prefix.apply(manifest).ok()?;
            return (prefix.latest_generation() == generation).then_some((previous, prefix));
        }
        prefix.apply(manifest).ok()?;
    }
    None
}
