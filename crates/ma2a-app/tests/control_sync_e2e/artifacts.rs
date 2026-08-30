use std::{
    error::Error,
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use iroh::{SecretKey, TransportAddr};
use ma2a_core::{
    AddressEndpointDataV1, AddressRecordScope, AddressRecordValidity,
    PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1, PrivateRelayAdvertisementValidity,
    SpaceAddressRecordV1, SpaceAuthorizationView,
};
use ma2a_net::{
    AddressRecordTarget, AddressRecordValidator, AdvertisementValidationContext,
    PrivateRelayAdvertisementValidator,
};
use ma2a_store::{KeyKind, KeyReference, KeyStore, Repository};

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
const NOW_MS: i64 = 1_700_000_000_000;

#[derive(Clone, Copy)]
pub(super) struct AddressFixture {
    pub(super) port: u16,
    pub(super) sequence: u64,
}

pub(super) fn endpoint_secret(state_dir: &Path) -> TestResultValue<SecretKey> {
    let reference = KeyReference::parse("endpoint-identity-v1")?;
    let protected = KeyStore::open(state_dir)?.read(KeyKind::Endpoint, &reference)?;
    let bytes = <[u8; 32]>::try_from(protected.as_ref())?;
    Ok(SecretKey::from_bytes(&bytes))
}

pub(super) fn address_records(
    spaces: [ma2a_core::SpaceId; 2],
    secret: &SecretKey,
    fixture: AddressFixture,
) -> TestResultValue<Vec<ma2a_core::SignedSpaceAddressRecordV1>> {
    spaces
        .into_iter()
        .map(|space_id| address_record(space_id, secret, fixture))
        .collect()
}

fn address_record(
    space_id: ma2a_core::SpaceId,
    secret: &SecretKey,
    fixture: AddressFixture,
) -> TestResultValue<ma2a_core::SignedSpaceAddressRecordV1> {
    Ok(SpaceAddressRecordV1::new(
        AddressRecordScope::new(space_id, secret.public().into()),
        AddressRecordValidity::new(
            fixture.sequence,
            u64::try_from(NOW_MS)?,
            u64::try_from(NOW_MS + 600_000)?,
        )?,
        AddressEndpointDataV1::new(vec![TransportAddr::Ip(SocketAddr::new(
            Ipv4Addr::LOCALHOST.into(),
            fixture.port,
        ))])?,
    )
    .sign(secret)?)
}

pub(super) fn relay_advertisement(
    space_id: ma2a_core::SpaceId,
    secret: &SecretKey,
) -> TestResultValue<ma2a_core::SignedPrivateRelayAdvertisementV1> {
    Ok(PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(space_id, secret.public().into()),
        "https://relay.example.invalid".parse()?,
        PrivateRelayAdvertisementValidity::new(
            1,
            u64::try_from(NOW_MS)?,
            u64::try_from(NOW_MS + 600_000)?,
        )?,
    )?
    .sign(secret)?)
}

pub(super) fn persist_address(
    repository: &mut Repository,
    record: &ma2a_core::SignedSpaceAddressRecordV1,
    now_ms: u64,
) -> TestResult {
    let chain = repository
        .load_space_chain(record.record().space_id())?
        .ok_or("address Space chain missing")?;
    let authorization = SpaceAuthorizationView::from_chain(&chain);
    let target =
        AddressRecordTarget::new(record.record().space_id(), record.record().endpoint_id());
    AddressRecordValidator::validate_and_store(
        repository,
        record.canonical_bytes(),
        target.validation(&authorization, now_ms),
    )?;
    Ok(())
}

pub(super) fn persist_relay(
    repository: &mut Repository,
    advertisement: &ma2a_core::SignedPrivateRelayAdvertisementV1,
    now_ms: u64,
) -> TestResult {
    let chain = repository
        .load_space_chain(advertisement.advertisement().space_id())?
        .ok_or("relay Space chain missing")?;
    let authorization = SpaceAuthorizationView::from_chain(&chain);
    PrivateRelayAdvertisementValidator::validate_and_store(
        repository,
        advertisement.canonical_bytes(),
        AdvertisementValidationContext::new(&authorization, now_ms),
    )?;
    Ok(())
}
