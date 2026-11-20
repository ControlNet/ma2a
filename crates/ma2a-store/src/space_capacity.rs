//! How many generations a locally owned Space may sign.
//!
//! A Space's public chain carries at most 255 manifests, and its departure and enrollment
//! answers page the same chain, so a generation beyond that could be committed but never
//! handed to anyone. The last generations are kept for revocations: a Space that reached its
//! limit through ordinary changes can still remove a member.

use ma2a_core::{EnrollmentPage, SpaceChain, SpaceManifestMembership};

use crate::StoreError;

/// Highest generation a public chain can carry.
const MAX_GENERATION: u64 = 255;
/// Generations below the maximum that only a revocation may use.
const REVOCATION_RESERVE: u64 = 16;

/// Whether `membership` revokes an Endpoint that `chain` has not revoked yet.
pub(crate) fn revokes(chain: &SpaceChain, membership: &SpaceManifestMembership) -> bool {
    membership.revocations().iter().any(|revocation| {
        !chain
            .revocations()
            .iter()
            .any(|known| known.endpoint_id() == revocation.endpoint_id())
    })
}

/// Refuses to sign `generation` unless it fits, keeping the reserve for revocations.
pub(crate) const fn admit(generation: u64, revocation: bool) -> Result<(), StoreError> {
    let limit = if revocation {
        MAX_GENERATION
    } else {
        MAX_GENERATION - REVOCATION_RESERVE
    };
    if generation <= limit {
        Ok(())
    } else {
        Err(StoreError::SpaceHistoryFull)
    }
}

/// Refuses a signed candidate chain that could not be exported or paged as a whole.
pub(crate) fn deliverable(candidate: &SpaceChain) -> Result<(), StoreError> {
    if candidate.export_public().is_ok() && EnrollmentPage::paginate(candidate).is_ok() {
        Ok(())
    } else {
        Err(StoreError::SpaceHistoryFull)
    }
}

#[cfg(test)]
mod tests {
    use super::admit;

    #[test]
    fn the_last_generations_are_kept_for_revocations() {
        assert!(admit(239, false).is_ok());
        assert!(admit(240, false).is_err());
        assert!(admit(240, true).is_ok());
        assert!(admit(255, true).is_ok());
        assert!(admit(256, true).is_err());
    }
}
