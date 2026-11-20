//! The page a peer pulls: manifests after its cursor, then the records it has not seen.

use ma2a_core::{ControlArtifactKind, ControlCursorV1, ControlPageV1};
use ma2a_net::{ControlRejection, is_unseen};
use ma2a_store::ControlSpaceState;

use super::{PullPageRequest, builder::PageBuilder, hash_at_generation};

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
