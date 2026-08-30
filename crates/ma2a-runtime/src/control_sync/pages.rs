mod builder;
mod staging;

use std::collections::BTreeSet;

use ma2a_core::{ControlArtifactKind, ControlCursorV1, ControlPageV1, SpaceChain};
use ma2a_net::{ControlRejection, cursor_sequence};
use ma2a_store::ControlSpaceState;

use crate::error::{RuntimeError, RuntimeErrorKind};
use builder::PageBuilder;
pub(crate) use staging::ControlChanges;
pub(super) use staging::apply_pages;

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
