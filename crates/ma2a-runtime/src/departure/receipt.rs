//! The chain a repeated departure is answered with.
//!
//! A member that already left may ask again, for example after losing the first answer. It is
//! owed the chain that removed it and nothing signed after that: later members, labels, and
//! grants belong to people it no longer shares the Space with.

use ma2a_core::{EndpointId, SpaceChain};

/// Returns `chain` cut at the latest generation that removed and revoked `departed`, or `None`
/// when no generation of `chain` did.
pub(crate) fn through_removal(chain: &SpaceChain, departed: EndpointId) -> Option<SpaceChain> {
    let is_member = |chain: &SpaceChain| {
        chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == departed)
    };
    let mut prefix = SpaceChain::from_genesis(chain.genesis().clone()).ok()?;
    let mut was_member = is_member(&prefix);
    let mut removal = None;
    for manifest in chain.manifests() {
        prefix.apply(manifest).ok()?;
        let member = is_member(&prefix);
        if was_member
            && !member
            && prefix
                .revocations()
                .iter()
                .any(|revocation| revocation.endpoint_id() == departed)
        {
            removal = Some(prefix.clone());
        }
        was_member = member;
    }
    removal
}

#[cfg(test)]
mod tests {
    use super::through_removal;
    use crate::departure::tests::{
        TestResult, endpoint, shared_space, with_departure, with_member,
    };

    #[test]
    fn a_repeated_departure_learns_nothing_signed_after_its_removal() -> TestResult {
        // Given: 0x42 left at generation 2, then 0x43 joined at generation 3.
        let (chain, secret) = shared_space(0x35)?;
        let departed = endpoint(0x42)?;
        let removed = with_departure(&chain, &secret, departed)?;
        let later = with_member(&removed, &secret, endpoint(0x43)?)?;

        // When
        let answer = through_removal(&later, departed).ok_or("no removal found")?;

        // Then: exactly the chain that removed it, without the later member.
        assert_eq!(answer, removed);
        assert_eq!(later.latest_generation(), answer.latest_generation() + 1);
        Ok(())
    }

    #[test]
    fn a_chain_that_never_removed_the_endpoint_has_no_answer() -> TestResult {
        // Given
        let (chain, _secret) = shared_space(0x36)?;

        // When / Then: 0x42 is still a member, and 0x44 never was one.
        assert!(through_removal(&chain, endpoint(0x42)?).is_none());
        assert!(through_removal(&chain, endpoint(0x44)?).is_none());
        Ok(())
    }
}
