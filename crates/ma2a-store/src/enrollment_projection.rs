use crate::{Committed, ControlBatch, Repository, StoreError};
use ma2a_core::{EndpointId, SpaceChain, SpaceId};
use std::collections::BTreeSet;

/// Membership and chain read in the same transaction that accepts a bootstrap.
#[derive(Debug)]
#[non_exhaustive]
pub struct EnrollmentProjection {
    /// The accepted durable chain read before committing the transaction.
    pub chain: SpaceChain,
    /// All memberships at this transaction revision.
    pub memberships: BTreeSet<SpaceId>,
}

impl Repository {
    /// Persists a bootstrap and derives its result before the transaction commits.
    ///
    /// # Errors
    /// Returns an error when validation, persistence, or the resulting projection fails.
    #[expect(
        clippy::too_many_arguments,
        reason = "bootstrap projection requires Space and local identity"
    )]
    pub fn persist_enrollment_batch(
        &mut self,
        batch: &ControlBatch,
        space_id: SpaceId,
        endpoint_id: EndpointId,
    ) -> Result<Committed<EnrollmentProjection>, StoreError> {
        self.persist_control_batch_projected(batch, |transaction| {
            Ok(EnrollmentProjection {
                chain: crate::space_rows::load_chain(transaction, space_id)?
                    .ok_or(StoreError::SpaceNotFound)?,
                memberships: crate::spaces::memberships_for(transaction, endpoint_id)?,
            })
        })
        .map(|(_, committed)| committed)
    }
}
