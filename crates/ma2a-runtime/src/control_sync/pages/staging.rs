use std::collections::BTreeMap;

use ma2a_core::{
    ControlArtifactKind, SignedSpaceAddressRecordV1, SignedSpaceManifestV1, SpaceAuthorizationView,
};
use ma2a_net::{
    AddressRecordTarget, AddressRecordValidator, AdvertisementValidationContext, ControlRejection,
    PrivateRelayAdvertisementValidator,
};
use ma2a_store::{ControlBatch, Repository, ValidatedAddressRecord, ValidatedRelayAdvertisement};

use super::PageApplication;
use crate::control_sync::ControlFailure;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ControlChanges {
    pub(crate) manifest: bool,
    pub(crate) address: bool,
    pub(crate) relay: bool,
}

impl ControlChanges {
    pub(crate) const fn merge(&mut self, other: Self) {
        self.manifest |= other.manifest;
        self.address |= other.address;
        self.relay |= other.relay;
    }

    pub(crate) fn triggers(self) -> impl Iterator<Item = crate::control_sync::ControlRoundTrigger> {
        [
            self.manifest
                .then_some(crate::control_sync::ControlRoundTrigger::ManifestAdvanced),
            self.address
                .then_some(crate::control_sync::ControlRoundTrigger::AddressAdvanced),
            self.relay
                .then_some(crate::control_sync::ControlRoundTrigger::RelayAdvanced),
        ]
        .into_iter()
        .flatten()
    }
}

pub(crate) fn apply_pages(
    repository: &mut Repository,
    application: PageApplication<'_>,
) -> Result<ControlChanges, ControlFailure> {
    let mut chains = Vec::new();
    let mut addresses = BTreeMap::new();
    let mut relays = BTreeMap::new();
    for page in application.pages {
        let state = application
            .shared
            .iter()
            .find(|state| state.chain().space_id() == page.space_id())
            .ok_or(ControlRejection::Invalid)?;
        let mut chain = state.chain().clone();
        for artifact in page
            .artifacts()
            .iter()
            .filter(|artifact| artifact.kind() == ControlArtifactKind::MANIFEST)
        {
            let manifest = SignedSpaceManifestV1::from_canonical_bytes(
                artifact.signed_bytes(),
                chain.genesis().authority(),
            )
            .map_err(|_| ControlRejection::Invalid)?;
            if manifest.generation() <= chain.latest_generation() {
                let index = manifest
                    .generation()
                    .checked_sub(1)
                    .and_then(|generation| usize::try_from(generation).ok())
                    .ok_or(ControlRejection::Invalid)?;
                let accepted = chain
                    .manifests()
                    .get(index)
                    .ok_or(ControlRejection::Invalid)?;
                if accepted.canonical_bytes() != manifest.canonical_bytes() {
                    return Err(ControlRejection::Invalid.into());
                }
                continue;
            }
            chain
                .apply(&manifest)
                .map_err(|_| ControlRejection::Invalid)?;
        }
        if &chain != state.chain() {
            chains.push(chain.clone());
        }
        let authorization = SpaceAuthorizationView::from_chain(&chain);
        for artifact in page.artifacts() {
            match artifact.kind() {
                kind if kind == ControlArtifactKind::MANIFEST => {}
                kind if kind == ControlArtifactKind::ADDRESS_RECORD => {
                    let signed =
                        SignedSpaceAddressRecordV1::parse_canonical_bytes(artifact.signed_bytes())
                            .map_err(|_| ControlRejection::Invalid)?;
                    let target =
                        AddressRecordTarget::new(page.space_id(), signed.record().endpoint_id());
                    let validated = AddressRecordValidator::validate(
                        artifact.signed_bytes(),
                        target.validation(&authorization, application.now_ms),
                    )
                    .map_err(|_| ControlRejection::Invalid)?;
                    stage_address(repository, &mut addresses, validated)?;
                }
                kind if kind == ControlArtifactKind::RELAY_ADVERTISEMENT => {
                    let validated = PrivateRelayAdvertisementValidator::validate(
                        artifact.signed_bytes(),
                        AdvertisementValidationContext::new(&authorization, application.now_ms),
                    )
                    .map_err(|_| ControlRejection::Invalid)?;
                    stage_relay(repository, &mut relays, validated)?;
                }
                _ => return Err(ControlRejection::Invalid.into()),
            }
        }
    }
    let changes = ControlChanges {
        manifest: !chains.is_empty(),
        address: !addresses.is_empty(),
        relay: !relays.is_empty(),
    };
    repository
        .persist_control_batch(&ControlBatch::new(
            chains,
            addresses.into_values().collect(),
            relays.into_values().collect(),
        ))
        .map_err(ControlFailure::from)?;
    Ok(changes)
}

fn stage_address(
    repository: &Repository,
    staged: &mut BTreeMap<(ma2a_core::SpaceId, ma2a_core::EndpointId), ValidatedAddressRecord>,
    advance: ValidatedAddressRecord,
) -> Result<(), ControlFailure> {
    let key = (
        advance.record().record().space_id(),
        advance.record().record().endpoint_id(),
    );
    let current = staged.get(&key).map_or_else(
        || {
            repository
                .address_record(key.0, key.1)
                .map_err(ControlFailure::from)
                .map(|record| {
                    record.map(|record| {
                        (
                            record.sequence(),
                            record.record_hash(),
                            record.signed_record().to_vec(),
                        )
                    })
                })
        },
        |record| {
            Ok(Some((
                record.record().record().sequence(),
                record.record().record_hash(),
                record.record().canonical_bytes().to_vec(),
            )))
        },
    )?;
    if let Some((sequence, hash, signed)) = current {
        match advance.record().record().sequence().cmp(&sequence) {
            std::cmp::Ordering::Less => return Err(ControlRejection::Invalid.into()),
            std::cmp::Ordering::Equal
                if hash != advance.record().record_hash()
                    || signed != advance.record().canonical_bytes() =>
            {
                return Err(ControlRejection::Invalid.into());
            }
            std::cmp::Ordering::Equal => return Ok(()),
            std::cmp::Ordering::Greater => {}
        }
    }
    staged.insert(key, advance);
    Ok(())
}

fn stage_relay(
    repository: &Repository,
    staged: &mut BTreeMap<(ma2a_core::SpaceId, ma2a_core::EndpointId), ValidatedRelayAdvertisement>,
    advance: ValidatedRelayAdvertisement,
) -> Result<(), ControlFailure> {
    let advertisement = advance.signed().advertisement();
    let key = (
        advertisement.space_id(),
        advertisement.provider_endpoint_id(),
    );
    let current = staged.get(&key).map_or_else(
        || {
            repository
                .relay_advertisement(key.0, key.1)
                .map_err(ControlFailure::from)
                .map(|record| {
                    record.map(|record| {
                        (
                            record.sequence(),
                            record.advertisement_hash(),
                            record.signed_advertisement().to_vec(),
                        )
                    })
                })
        },
        |record| {
            Ok(Some((
                record.signed().advertisement().sequence(),
                record.signed().advertisement_hash(),
                record.signed().canonical_bytes().to_vec(),
            )))
        },
    )?;
    if let Some((sequence, hash, signed)) = current {
        match advertisement.sequence().cmp(&sequence) {
            std::cmp::Ordering::Less => return Err(ControlRejection::Invalid.into()),
            std::cmp::Ordering::Equal
                if hash != advance.signed().advertisement_hash()
                    || signed != advance.signed().canonical_bytes() =>
            {
                return Err(ControlRejection::Invalid.into());
            }
            std::cmp::Ordering::Equal => return Ok(()),
            std::cmp::Ordering::Greater => {}
        }
    }
    staged.insert(key, advance);
    Ok(())
}
