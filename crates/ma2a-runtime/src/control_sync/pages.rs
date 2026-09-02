mod builder;
mod staging;

use std::collections::BTreeSet;

use ma2a_core::{ControlArtifactKind, ControlCursorV1, ControlPageV1, SpaceChain};
use ma2a_net::{ControlRejection, cursor_sequence};
use ma2a_store::ControlSpaceState;

use crate::error::{RuntimeError, RuntimeErrorKind};
use builder::{PAGE_HEADER_BYTES, PageBuilder};
pub(crate) use staging::ControlChanges;
pub(super) use staging::apply_pages;

#[derive(Clone, Copy)]
pub(super) struct PushPageRequest {
    pub(super) local_endpoint_id: ma2a_core::EndpointId,
    pub(super) now_ms: u64,
    pub(super) artifact_budget: usize,
}

#[derive(Clone, Copy)]
pub(super) struct PullPageRequest {
    pub(super) now_ms: u64,
    pub(super) artifact_budget: usize,
}

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
    request: PushPageRequest,
) -> Result<ControlPageV1, RuntimeError> {
    let mut page = PageBuilder::with_budget(request.artifact_budget);
    let mut omitted = false;
    for manifest in state.chain().manifests() {
        omitted |= !page.add(ControlArtifactKind::MANIFEST, manifest.canonical_bytes())?;
    }
    for record in state.address_records() {
        if record.endpoint_id() == request.local_endpoint_id
            && u64::try_from(record.issued_at_ms()).is_ok_and(|issued| issued <= request.now_ms)
            && u64::try_from(record.expires_at_ms()).is_ok_and(|expires| expires > request.now_ms)
        {
            omitted |= !page.add(ControlArtifactKind::ADDRESS_RECORD, record.signed_record())?;
        }
    }
    for advertisement in state.active_relay_advertisements() {
        if advertisement.provider_endpoint_id() == request.local_endpoint_id
            && u64::try_from(advertisement.issued_at_ms())
                .is_ok_and(|issued| issued <= request.now_ms)
            && u64::try_from(advertisement.expires_at_ms())
                .is_ok_and(|expires| expires > request.now_ms)
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
    request: PullPageRequest,
) -> Result<ControlPageV1, ControlRejection> {
    if cursor.manifest_generation() > state.chain().latest_generation()
        || hash_at_generation(state.chain(), cursor.manifest_generation())?
            != cursor.manifest_hash()
    {
        return Err(ControlRejection::Invalid);
    }
    let start =
        usize::try_from(cursor.manifest_generation()).map_err(|_| ControlRejection::Invalid)?;
    let mut page = PageBuilder::with_budget(request.artifact_budget);
    let mut omitted = false;
    for manifest in state.chain().manifests().iter().skip(start) {
        omitted |= !page.add_control(ControlArtifactKind::MANIFEST, manifest.canonical_bytes())?;
    }
    for record in state.address_records() {
        if record.sequence() > cursor_sequence(cursor.address_cursors(), record.endpoint_id())
            && u64::try_from(record.issued_at_ms()).is_ok_and(|issued| issued <= request.now_ms)
            && u64::try_from(record.expires_at_ms()).is_ok_and(|expires| expires > request.now_ms)
        {
            omitted |=
                !page.add_control(ControlArtifactKind::ADDRESS_RECORD, record.signed_record())?;
        }
    }
    for advertisement in state.active_relay_advertisements() {
        if advertisement.sequence()
            > cursor_sequence(cursor.relay_cursors(), advertisement.provider_endpoint_id())
            && u64::try_from(advertisement.issued_at_ms())
                .is_ok_and(|issued| issued <= request.now_ms)
            && u64::try_from(advertisement.expires_at_ms())
                .is_ok_and(|expires| expires > request.now_ms)
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

pub(super) fn response_artifact_budget(page_count: usize) -> Result<usize, ControlRejection> {
    let encoded = ma2a_core::ControlResponseV1::new(Vec::new())
        .and_then(|response| response.encode())
        .map_err(|_| ControlRejection::Invalid)?;
    artifact_budget(encoded.len(), page_count)
}

pub(super) fn request_artifact_budget(
    cursors: &[ControlCursorV1],
    page_count: usize,
) -> Result<usize, ControlRejection> {
    let encoded = ma2a_core::ControlRequestV1::new(cursors.to_vec())
        .and_then(|request| request.encode())
        .map_err(|_| ControlRejection::Invalid)?;
    artifact_budget(encoded.len(), page_count)
}

fn artifact_budget(
    encoded_without_pages: usize,
    page_count: usize,
) -> Result<usize, ControlRejection> {
    if page_count == 0 {
        return Ok(0);
    }
    ma2a_core::MAX_CONTROL_BATCH_BYTES
        .checked_sub(encoded_without_pages)
        .and_then(|bytes| bytes.checked_sub(PAGE_HEADER_BYTES.checked_mul(page_count)?))
        .map(|bytes| bytes / page_count)
        .ok_or(ControlRejection::Invalid)
}

#[cfg(test)]
#[expect(
    clippy::items_after_test_module,
    reason = "codec tests remain adjacent to their private budget helpers"
)]
mod codec_budget_tests {
    use ma2a_core::{
        ControlArtifactKind, ControlCursorV1, ControlPageV1, ControlRequestV1, ControlResponseV1,
        MAX_CONTROL_BATCH_BYTES, SpaceId,
    };

    use super::{request_artifact_budget, response_artifact_budget};
    use crate::control_sync::pages::builder::PageBuilder;

    #[test]
    fn two_response_pages_fit_the_production_codec_limit() -> Result<(), ma2a_core::ProtocolError> {
        // Given
        let budget = response_artifact_budget(2).expect("two response pages have a budget");
        let pages = build_pages(budget)?;

        // When
        let encoded = ControlResponseV1::new(pages)?.encode()?;

        // Then
        assert!(encoded.len() <= MAX_CONTROL_BATCH_BYTES);
        Ok(())
    }

    #[test]
    fn two_push_pages_fit_the_production_codec_limit() -> Result<(), ma2a_core::ProtocolError> {
        // Given
        let cursors = vec![cursor(0x81)?, cursor(0x82)?];
        let budget = request_artifact_budget(&cursors, 2).expect("two push pages have a budget");
        let pages = build_pages(budget)?;

        // When
        let encoded = ControlRequestV1::new(cursors)?
            .with_push_pages(pages)?
            .encode()?;

        // Then
        assert!(encoded.len() <= MAX_CONTROL_BATCH_BYTES);
        Ok(())
    }

    fn cursor(seed: u8) -> Result<ControlCursorV1, ma2a_core::ProtocolError> {
        Ok(ControlCursorV1::new(
            SpaceId::try_from([seed; 32].as_slice())?,
            0,
            [0; 32],
        ))
    }

    fn build_pages(budget: usize) -> Result<Vec<ControlPageV1>, ma2a_core::ProtocolError> {
        [0x81_u8, 0x82]
            .into_iter()
            .map(|seed| {
                let mut builder = PageBuilder::with_budget(budget);
                let payload = vec![seed; budget.saturating_sub(5)];
                assert!(
                    builder
                        .add_control(ControlArtifactKind::MANIFEST, &payload)
                        .expect("budgeted artifact is accepted")
                );
                ControlPageV1::new(
                    SpaceId::try_from([seed; 32].as_slice())?,
                    false,
                    builder.artifacts,
                )
            })
            .collect()
    }
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
