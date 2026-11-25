//! The page a peer pulls: manifests after its cursor, then the records it has not seen.

use ma2a_core::{ControlArtifactKind, ControlCursorV1, ControlPageV1};
use ma2a_net::{ControlRejection, is_unseen};
use ma2a_store::ControlSpaceState;

use super::{
    PullPageRequest,
    builder::{ARTIFACT_OVERHEAD, PageBuilder},
    hash_at_generation, response_artifact_budget,
};

/// Builds one response page for each requested Space, in request order.
///
/// Every page first gets an equal share of the response. What short pages leave of their share
/// then goes, in request order, to the pages that still had more to send. An equal share of a
/// 64-Space request is smaller than a full address record, so without this such a record would
/// be held back on every round even when every other page is empty.
pub(in crate::control_sync) fn pull_pages(
    requested: &[(&ControlSpaceState, &ControlCursorV1)],
    now_ms: u64,
) -> Result<Vec<ControlPageV1>, ControlRejection> {
    let share = response_artifact_budget(requested.len())?;
    let pull = |(state, cursor): &(&ControlSpaceState, &ControlCursorV1), artifact_budget| {
        pull_page(
            state,
            cursor,
            PullPageRequest {
                now_ms,
                artifact_budget,
            },
        )
    };
    let mut pages = requested
        .iter()
        .map(|space| pull(space, share))
        .collect::<Result<Vec<_>, _>>()?;
    let used = pages.iter().map(page_bytes).sum::<usize>();
    let mut unused = (share * requested.len()).saturating_sub(used);
    for (space, page) in requested.iter().zip(&mut pages) {
        if page.has_more() && unused > 0 {
            let budget = page_bytes(page) + unused;
            *page = pull(space, budget)?;
            unused = budget.saturating_sub(page_bytes(page));
        }
    }
    Ok(pages)
}

/// The bytes a page's artifacts take in its encoding, as [`PageBuilder`] counts them.
fn page_bytes(page: &ControlPageV1) -> usize {
    page.artifacts()
        .iter()
        .map(|artifact| artifact.signed_bytes().len() + ARTIFACT_OVERHEAD)
        .sum()
}

pub(in crate::control_sync) fn pull_page(
    state: &ControlSpaceState,
    cursor: &ControlCursorV1,
    request: PullPageRequest,
) -> Result<ControlPageV1, ControlRejection> {
    if cursor.manifest_generation() > state.chain().latest_generation() {
        // The requester holds a longer chain, for example one whose pushed prefix this Runtime
        // just committed. It has nothing to learn from here yet; this Runtime pulls the rest in
        // its own rounds. Nothing else is sent, because the requester validates records against
        // its own, later chain.
        return ControlPageV1::new(state.chain().space_id(), false, Vec::new())
            .map_err(|_| ControlRejection::Invalid);
    }
    if hash_at_generation(state.chain(), cursor.manifest_generation())? != cursor.manifest_hash() {
        return Err(ControlRejection::Invalid);
    }
    let start =
        usize::try_from(cursor.manifest_generation()).map_err(|_| ControlRejection::Invalid)?;
    let mut page = PageBuilder::with_budget(request.artifact_budget);
    let mut omitted = !page.add_contiguous(
        ControlArtifactKind::MANIFEST,
        state
            .chain()
            .manifests()
            .iter()
            .skip(start)
            .map(ma2a_core::SignedSpaceManifestV1::canonical_bytes),
    )?;
    if omitted {
        // The receiver validates every record against the chain this page brings it to; with
        // a generation still missing that is not the latest chain, so records wait a page.
        return ControlPageV1::new(state.chain().space_id(), omitted, page.artifacts)
            .map_err(|_| ControlRejection::Invalid);
    }
    // A removed member's record stays stored until it expires, but the receiver rejects the
    // whole page for it, which would hold back the very revocation that removed it.
    let authorization = state.authorization();
    for record in state.address_records() {
        if authorization.contains_member(record.endpoint_id())
            && super::is_exportable_record(record)
            && is_unseen(
                cursor.address_cursors(),
                (record.endpoint_id(), record.sequence()),
            )
            && u64::try_from(record.issued_at_ms()).is_ok_and(|issued| issued <= request.now_ms)
            && u64::try_from(record.expires_at_ms()).is_ok_and(|expires| expires > request.now_ms)
        {
            omitted |=
                !page.add_control(ControlArtifactKind::ADDRESS_RECORD, record.signed_record())?;
        }
    }
    for advertisement in state.active_relay_advertisements() {
        if authorization.allows_private_relay_provider(advertisement.provider_endpoint_id())
            && super::is_exportable(advertisement)
            && is_unseen(
                cursor.relay_cursors(),
                (
                    advertisement.provider_endpoint_id(),
                    advertisement.sequence(),
                ),
            )
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
