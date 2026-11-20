//! How many generations a locally owned Space may sign.
//!
//! A Space's public chain carries at most 255 manifests, and its departure and enrollment
//! answers page the same chain, so a generation beyond that could be committed but never
//! handed to anyone. The last generations are kept for revocations: a Space that reached its
//! limit through ordinary changes can still remove a member.
//!
//! Beyond that reserve, every change but a removal must leave each member removable on its
//! own: one generation and one revocation entry per non-owner member must still be free. A
//! removal revokes and keeps only some of the members it had, adding none, so it frees as much
//! as it uses; it stays allowed for a history signed before this rule that already broke it.

use std::collections::BTreeSet;

use ma2a_core::{EnrollmentPage, MAX_SPACE_MEMBERS, SpaceChain, SpaceManifestMembership};

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

/// Refuses to sign `membership` as `generation` of `chain` unless it fits, keeping the reserve
/// for revocations and every non-owner member removable on its own.
pub(crate) fn admit(
    chain: &SpaceChain,
    generation: u64,
    membership: &SpaceManifestMembership,
) -> Result<(), StoreError> {
    let owner = chain.genesis().genesis().initial_member().endpoint_id();
    let others = |members: &[ma2a_core::SpaceMemberV1]| {
        members
            .iter()
            .map(ma2a_core::SpaceMemberV1::endpoint_id)
            .filter(|endpoint| *endpoint != owner)
            .collect::<BTreeSet<_>>()
    };
    let (before, after) = (others(chain.members()), others(membership.members()));
    // Comparing sets, not counts: a change that swaps one member for another is no removal.
    let removal =
        revokes(chain, membership) && after.len() < before.len() && after.is_subset(&before);
    fits(
        generation,
        removal,
        (after.len(), membership.revocations().len()),
    )
}

/// Whether `generation` fits; outside a `removal`, also whether `others` non-owner members
/// beside `revoked` revocation entries can each still be removed by a later generation.
const fn fits(
    generation: u64,
    removal: bool,
    (others, revoked): (usize, usize),
) -> Result<(), StoreError> {
    let limit = if removal {
        MAX_GENERATION
    } else {
        MAX_GENERATION - REVOCATION_RESERVE
    };
    let remaining = MAX_GENERATION.saturating_sub(generation);
    let removable = removal
        || ((others as u64) <= remaining && revoked.saturating_add(others) <= MAX_SPACE_MEMBERS);
    if generation <= limit && removable {
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
    use super::fits;

    #[test]
    fn the_last_generations_are_kept_for_revocations() {
        assert!(fits(239, false, (0, 0)).is_ok());
        assert!(fits(240, false, (0, 0)).is_err());
        assert!(fits(240, true, (0, 0)).is_ok());
        assert!(fits(255, true, (0, 0)).is_ok());
        assert!(fits(256, true, (0, 0)).is_err());
    }

    #[test]
    fn a_change_keeps_a_generation_free_for_removing_each_member() {
        // Generation 239 leaves 16 generations: 16 members can each still be removed.
        assert!(fits(239, false, (16, 0)).is_ok());
        assert!(fits(239, false, (17, 0)).is_err());
        assert!(fits(200, false, (55, 0)).is_ok());
        assert!(fits(200, false, (56, 0)).is_err());
        // A removal is allowed even where an older history already broke the rule.
        assert!(fits(240, true, (40, 0)).is_ok());
    }

    #[test]
    fn a_change_keeps_a_revocation_entry_free_for_removing_each_member() {
        assert!(fits(10, false, (1, 63)).is_ok());
        assert!(fits(10, false, (1, 64)).is_err());
        assert!(fits(10, false, (32, 32)).is_ok());
        assert!(fits(10, false, (33, 32)).is_err());
        assert!(fits(10, true, (1, 64)).is_ok());
    }
}
