use iroh::SecretKey;
use ma2a_core::{
    MemberCapabilities, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SpaceAuthorizationView, SpaceManifestMembership,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{AdvertisementValidationContext, PrivateRelayAdvertisementValidator, RelayUrl};
use ma2a_store::{OwnedSpaceUpdate, Repository, SpaceCreation};

use super::{NOW_MS, TestResult};

pub(super) fn create_space_with_relay(
    repository: &mut Repository,
    local_endpoint_id: ma2a_core::EndpointId,
    relay_endpoint_id: ma2a_core::EndpointId,
) -> Result<SpaceAuthorizationView, Box<dyn std::error::Error + Send + Sync>> {
    let local = SpaceMemberV1::new(
        local_endpoint_id,
        "local".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let relay = SpaceMemberV1::new(
        relay_endpoint_id,
        "relay".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let created = repository.create_owned_space(&SpaceCreation::new(
        10,
        local.clone(),
        SpacePolicyV1::phase_one_default(),
    ))?;
    let mut members = vec![local, relay];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    repository.advance_owned_space(&OwnedSpaceUpdate::new(
        created.space_id(),
        11,
        SpaceManifestMembership::new(members, Vec::new()),
    ))?;
    let chain = repository
        .load_space_chain(created.space_id())?
        .ok_or("created Space chain is missing")?;
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

pub(super) struct AdvertisementFixture<'a> {
    pub(super) authorization: &'a SpaceAuthorizationView,
    pub(super) relay: &'a SecretKey,
    pub(super) relay_url: RelayUrl,
}

pub(super) fn store_advertisement(
    repository: &mut Repository,
    fixture: AdvertisementFixture<'_>,
) -> TestResult {
    let now_ms = u64::try_from(NOW_MS)?;
    let signed = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(
            fixture.authorization.space_id(),
            fixture.relay.public().into(),
        ),
        fixture.relay_url,
        PrivateRelayAdvertisementValidity::new(1, now_ms, now_ms + 600_000)?,
    )?
    .sign(fixture.relay)?;
    PrivateRelayAdvertisementValidator::validate_and_store(
        repository,
        signed.canonical_bytes(),
        AdvertisementValidationContext::new(fixture.authorization, now_ms),
    )?;
    Ok(())
}
