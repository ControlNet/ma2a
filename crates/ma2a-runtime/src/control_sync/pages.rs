mod builder;

use std::collections::BTreeSet;

use ma2a_core::{
    ControlArtifactKind, ControlCursorV1, ControlPageV1, SignedSpaceAddressRecordV1,
    SignedSpaceManifestV1, SpaceAuthorizationView, SpaceChain,
};
use ma2a_net::{
    AddressRecordTarget, AddressRecordValidator, AdvertisementValidationContext, ControlRejection,
    PrivateRelayAdvertisementValidator, cursor_sequence,
};
use ma2a_store::{ControlSpaceState, Repository};

use crate::error::{RuntimeError, RuntimeErrorKind};
use builder::PageBuilder;

#[derive(Clone, Copy)]
pub(super) struct PageApplication<'a> {
    shared: &'a [ControlSpaceState],
    pages: &'a [ControlPageV1],
    now_ms: u64,
}

impl<'a> PageApplication<'a> {
    pub(super) const fn new(
        shared: &'a [ControlSpaceState],
        pages: &'a [ControlPageV1],
        now_ms: u64,
    ) -> Self {
        Self {
            shared,
            pages,
            now_ms,
        }
    }
}

pub(super) fn push_page(
    state: &ControlSpaceState,
    now_ms: u64,
) -> Result<ControlPageV1, RuntimeError> {
    let mut page = PageBuilder::new();
    let mut omitted = false;
    for manifest in state.chain().manifests() {
        omitted |= !page.add(ControlArtifactKind::MANIFEST, manifest.canonical_bytes())?;
    }
    for record in state.address_records() {
        if u64::try_from(record.issued_at_ms()).is_ok_and(|issued| issued <= now_ms)
            && u64::try_from(record.expires_at_ms()).is_ok_and(|expires| expires > now_ms)
        {
            omitted |= !page.add(ControlArtifactKind::ADDRESS_RECORD, record.signed_record())?;
        }
    }
    for advertisement in state.relay_advertisements() {
        if u64::try_from(advertisement.issued_at_ms()).is_ok_and(|issued| issued <= now_ms)
            && u64::try_from(advertisement.expires_at_ms()).is_ok_and(|expires| expires > now_ms)
        {
            omitted |= !page.add(
                ControlArtifactKind::RELAY_ADVERTISEMENT,
                advertisement.signed_advertisement(),
            )?;
        }
    }
    ControlPageV1::new(state.chain().space_id(), omitted, page.artifacts)
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))
}

pub(super) fn pull_page(
    state: &ControlSpaceState,
    cursor: &ControlCursorV1,
    now_ms: u64,
) -> Result<ControlPageV1, ControlRejection> {
    if cursor.manifest_generation() > state.chain().latest_generation()
        || hash_at_generation(state.chain(), cursor.manifest_generation())?
            != cursor.manifest_hash()
    {
        return Err(ControlRejection::Invalid);
    }
    let start =
        usize::try_from(cursor.manifest_generation()).map_err(|_| ControlRejection::Invalid)?;
    let mut page = PageBuilder::new();
    let mut omitted = false;
    for manifest in state.chain().manifests().iter().skip(start) {
        omitted |= !page.add_control(ControlArtifactKind::MANIFEST, manifest.canonical_bytes())?;
    }
    for record in state.address_records() {
        if record.sequence() > cursor_sequence(cursor.address_cursors(), record.endpoint_id())
            && u64::try_from(record.issued_at_ms()).is_ok_and(|issued| issued <= now_ms)
            && u64::try_from(record.expires_at_ms()).is_ok_and(|expires| expires > now_ms)
        {
            omitted |=
                !page.add_control(ControlArtifactKind::ADDRESS_RECORD, record.signed_record())?;
        }
    }
    for advertisement in state.relay_advertisements() {
        if advertisement.sequence()
            > cursor_sequence(cursor.relay_cursors(), advertisement.provider_endpoint_id())
            && u64::try_from(advertisement.issued_at_ms()).is_ok_and(|issued| issued <= now_ms)
            && u64::try_from(advertisement.expires_at_ms()).is_ok_and(|expires| expires > now_ms)
        {
            omitted |= !page.add_control(
                ControlArtifactKind::RELAY_ADVERTISEMENT,
                advertisement.signed_advertisement(),
            )?;
        }
    }
    ControlPageV1::new(state.chain().space_id(), omitted, page.artifacts)
        .map_err(|_| ControlRejection::Invalid)
}

pub(super) fn apply_pages(
    repository: &mut Repository,
    application: PageApplication<'_>,
) -> Result<(), ControlRejection> {
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
                    return Err(ControlRejection::Invalid);
                }
                continue;
            }
            chain
                .apply(&manifest)
                .map_err(|_| ControlRejection::Invalid)?;
        }
        repository
            .persist_space_chain(&chain)
            .map_err(|_| ControlRejection::Unavailable)?;
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
                    AddressRecordValidator::validate_and_store(
                        repository,
                        artifact.signed_bytes(),
                        target.validation(&authorization, application.now_ms),
                    )
                    .map_err(|_| ControlRejection::Invalid)?;
                }
                kind if kind == ControlArtifactKind::RELAY_ADVERTISEMENT => {
                    PrivateRelayAdvertisementValidator::validate_and_store(
                        repository,
                        artifact.signed_bytes(),
                        AdvertisementValidationContext::new(&authorization, application.now_ms),
                    )
                    .map_err(|_| ControlRejection::Invalid)?;
                }
                _ => return Err(ControlRejection::Invalid),
            }
        }
    }
    Ok(())
}

pub(super) fn validate_page_spaces(
    shared: &[ControlSpaceState],
    pages: &[ControlPageV1],
) -> Result<(), ControlRejection> {
    let shared_ids = shared
        .iter()
        .map(|state| state.chain().space_id())
        .collect::<BTreeSet<_>>();
    if pages
        .iter()
        .any(|page| !shared_ids.contains(&page.space_id()))
    {
        return Err(ControlRejection::Invalid);
    }
    Ok(())
}

pub(super) fn validate_cursor_spaces(
    shared: &[ControlSpaceState],
    cursors: &[ControlCursorV1],
) -> Result<(), ControlRejection> {
    let shared_ids = shared
        .iter()
        .map(|state| state.chain().space_id())
        .collect::<BTreeSet<_>>();
    if cursors
        .iter()
        .any(|cursor| !shared_ids.contains(&cursor.space_id()))
    {
        return Err(ControlRejection::Invalid);
    }
    Ok(())
}

fn hash_at_generation(chain: &SpaceChain, generation: u64) -> Result<[u8; 32], ControlRejection> {
    if generation == chain.latest_generation() {
        return Ok(chain.latest_hash());
    }
    let mut prefix =
        SpaceChain::from_genesis(chain.genesis().clone()).map_err(|_| ControlRejection::Invalid)?;
    let count = usize::try_from(generation).map_err(|_| ControlRejection::Invalid)?;
    for manifest in chain.manifests().iter().take(count) {
        prefix
            .apply(manifest)
            .map_err(|_| ControlRejection::Invalid)?;
    }
    Ok(prefix.latest_hash())
}
