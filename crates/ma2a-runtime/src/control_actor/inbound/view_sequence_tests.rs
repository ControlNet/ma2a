use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ma2a_core::{MemberCapabilities, SpaceManifestMembership, SpaceMemberV1, default_member_label};
use ma2a_net::EndpointSecret;
use ma2a_store::{OwnedSpaceUpdate, StoreConfig};
use tokio_stream::StreamExt as _;

use crate::Runtime;

mod enrollment_clear;
mod support;

use support::{
    MemberRouter, TempState, TestResult, endpoint_hex, enroll_pair, json_u64, sync_peers,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_inbound_manifest_clear_advances_view_and_invalidates_sse() -> TestResult {
    let owner_dir = TempState::new("stale-owner")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let member = MemberRouter::start("stale-member").await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    let member_id = member.runtime.handle().status().await?.endpoint_id();
    let (space_id, _, _) = enroll_pair(&owner, &member.runtime, 0x92).await?;

    // Gate member's next inbound control completion after its Store commit.
    let (inbound_entered, inbound_release) = super::arm_inbound_control_gate(member_id);
    let peer_c = EndpointSecret::parse(&[0x66; 32])?.endpoint_id();
    let caps = MemberCapabilities::new(true, true);
    let mut members = vec![
        SpaceMemberV1::new(owner_id, default_member_label(owner_id), caps)?,
        SpaceMemberV1::new(member_id, default_member_label(member_id), caps)?,
        SpaceMemberV1::new(peer_c, "peer-c".to_owned(), caps)?,
    ];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let now_ms = u64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    owner
        .handle()
        .advance_owned_space(OwnedSpaceUpdate::new(
            space_id,
            now_ms,
            SpaceManifestMembership::new(members, Vec::new()),
        ))
        .await?;
    tokio::time::timeout(Duration::from_secs(5), inbound_entered).await??;

    // Advance member's membership generation and re-synchronize owner so the paused
    // inbound completion becomes stale while owner is in member's synchronized peer set.
    member
        .runtime
        .handle()
        .create_owned_space("bump-gen".to_owned())
        .await?;
    member.runtime.handle().sync_control_with(owner_id).await?;

    let (boot_id, mid_snap) = member.fetch_snapshot().await?;
    let mid_rev = json_u64(&mid_snap, "revision")?;
    let mid_view = json_u64(&mid_snap, "view_sequence")?;
    assert_eq!(sync_peers(&mid_snap)?, vec![endpoint_hex(owner_id)]);
    let mut sse_body = member.open_sse(&boot_id, (mid_rev, mid_view)).await?;

    // Hold any follow-up outgoing round so invalidation must come from the inbound clear itself.
    let (round_entered, round_release) = member.runtime.handle().arm_control_round_gate();
    let _ = inbound_release.send(());
    tokio::time::timeout(Duration::from_secs(5), round_entered).await??;

    let (_, after_snap) = member.fetch_snapshot().await?;
    assert!(sync_peers(&after_snap)?.is_empty());
    assert_eq!(json_u64(&after_snap, "revision")?, mid_rev);
    let after_view = json_u64(&after_snap, "view_sequence")?;
    assert!(
        after_view > mid_view,
        "stale inbound peer clear left view_sequence unchanged ({mid_view} -> {after_view})"
    );

    let event = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match sse_body.next().await {
                Some(Ok(chunk)) => {
                    let text = String::from_utf8_lossy(&chunk);
                    if text.contains("snapshot_invalidated") || text.contains("resync-required") {
                        return Ok::<String, String>(text.into_owned());
                    }
                }
                Some(Err(error)) => return Err(format!("sse error: {error}")),
                None => return Err("sse stream closed without invalidation".to_owned()),
            }
        }
    })
    .await
    .map_err(|_| "timed out waiting for SSE invalidation after stale inbound clear")??;
    assert!(event.contains("snapshot_invalidated") || event.contains("resync-required"));

    drop(sse_body);
    let _ = round_release.send(());
    member.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}
