use ma2a_core::default_member_label;
use ma2a_store::StoreConfig;

use super::support::{TempState, TestResult, endpoint_hex, enroll_pair, sync_peers};
use crate::{EnrollmentAttempt, Runtime};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_request_enrollment_clear_advances_view_sequence() -> TestResult {
    let owner_dir = TempState::new("enroll-owner")?;
    let member_dir = TempState::new("enroll-member")?;
    let owner = Runtime::start(StoreConfig::new(&owner_dir.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    let member_id = member.handle().status().await?.endpoint_id();
    let (_, ticket, request_id) = enroll_pair(&owner, &member, 0x93).await?;

    let before = member.handle().snapshot().await?;
    let before_rev = before.revision();
    let before_view = before.view_sequence();
    assert_eq!(
        sync_peers(&before.to_value())?,
        vec![endpoint_hex(owner_id)]
    );

    let (_round_entered, round_release) = member.handle().arm_control_round_gate();
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            request_id,
            default_member_label(member_id),
        ))
        .await?;

    let after = member.handle().snapshot().await?;
    assert!(sync_peers(&after.to_value())?.is_empty());
    assert_eq!(after.revision(), before_rev);
    assert!(
        after.view_sequence() > before_view,
        "same-request enrollment peer clear left view_sequence unchanged ({before_view} -> {})",
        after.view_sequence()
    );

    let _ = round_release.send(());
    member.shutdown().await?;
    owner.shutdown().await?;
    Ok(())
}
