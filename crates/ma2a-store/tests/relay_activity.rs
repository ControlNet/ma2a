//! Durable relay advertisement activity and high-water coverage.

#[path = "common/support.rs"]
mod support;

use iroh_base::SecretKey;
use ma2a_core::{
    MemberCapabilities, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SpaceAuthorizationView, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{
    RelayAdvertisementOutcome, Repository, SpaceCreation, StoreConfig, ValidatedRelayAdvertisement,
};
use support::{TempState, TestResult};

const NOW_MS: u64 = 15;

#[test]
fn withdrawal_hides_advertisement_and_preserves_high_water_after_reopen() -> TestResult {
    // Given
    let state = TempState::new("relay-activity")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let provider = SecretKey::from_bytes(&[0x51; 32]);
    let member = SpaceMemberV1::new(
        provider.public().into(),
        "relay-provider".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        member,
        SpacePolicyV1::phase_one_default(),
    ))?;
    let authorization = SpaceAuthorizationView::from_chain(
        &repository
            .load_space_chain(created.space_id())?
            .ok_or("relay Space chain missing")?,
    );
    let relay = RelayFixture {
        provider: &provider,
        authorization: &authorization,
    };
    let accepted = relay.advertisement(5, "a")?;
    assert!(matches!(
        repository.advance_private_relay_advertisement(&accepted)?,
        RelayAdvertisementOutcome::Advanced { .. }
    ));
    assert_eq!(
        repository
            .control_spaces_for(provider.public().into())?
            .first()
            .ok_or("relay control state missing")?
            .active_relay_advertisements()
            .count(),
        1
    );

    // When
    let (_revision, changed) =
        repository.reconcile_private_relay_activity(provider.public().into(), &[])?;
    assert!(changed);
    drop(repository);
    let mut repository = Repository::open(&config)?;

    // Then
    let state = repository
        .control_spaces_for(provider.public().into())?
        .into_iter()
        .next()
        .ok_or("reopened relay control state missing")?;
    assert_eq!(state.active_relay_advertisements().count(), 0);
    let persisted = repository
        .relay_advertisement(created.space_id(), provider.public().into())?
        .ok_or("withdrawn relay high-water missing")?;
    assert!(!persisted.is_active());
    assert_eq!(persisted.sequence(), 5);
    assert_eq!(
        repository.advance_private_relay_advertisement(&relay.advertisement(4, "rollback")?)?,
        RelayAdvertisementOutcome::Rollback {
            current_sequence: 5
        }
    );
    assert_eq!(
        repository.advance_private_relay_advertisement(&relay.advertisement(5, "fork")?)?,
        RelayAdvertisementOutcome::Fork {
            current_sequence: 5
        }
    );
    Ok(())
}

struct RelayFixture<'a> {
    provider: &'a SecretKey,
    authorization: &'a SpaceAuthorizationView,
}

impl RelayFixture<'_> {
    fn advertisement(
        &self,
        sequence: u64,
        suffix: &str,
    ) -> Result<ValidatedRelayAdvertisement, Box<dyn std::error::Error + Send + Sync>> {
        let signed = PrivateRelayAdvertisementV1::new(
            PrivateRelayAdvertisementScope::new(
                self.authorization.space_id(),
                self.provider.public().into(),
            ),
            format!("https://relay-{suffix}.example.invalid").parse()?,
            PrivateRelayAdvertisementValidity::new(sequence, 10, 20)?,
        )?
        .sign(self.provider)?;
        ValidatedRelayAdvertisement::parse(signed.canonical_bytes(), self.authorization, NOW_MS)
            .map_err(|_| "relay advertisement validation failed".into())
    }
}
