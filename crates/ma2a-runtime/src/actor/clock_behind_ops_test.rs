//! Control sync and authority-side departure still succeed while the local clock is stepped back.
use std::{
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
    time::Duration,
};

use ma2a_core::{InviteEntropy, RequestId};
use ma2a_store::{Repository, StoreConfig, StoreError};
use tokio::time::timeout;

use super::{
    maintenance::FaultPoint,
    relay_lifecycle_test::{NOW_MS, TempState},
};
use crate::{
    EnrollmentAttempt, EnrollmentCreation, Runtime, RuntimeClock, error::RuntimeError,
    state::RuntimeEvent,
};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const PROMPT: Duration = Duration::from_secs(10);

mod enrollment_test;
mod mutations_test;
mod observation_test;
mod relay_mutations_test;

#[derive(Debug)]
struct MutableClock(AtomicI64);

impl RuntimeClock for MutableClock {
    fn now_ms(&self) -> Result<i64, RuntimeError> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_sync_and_departure_succeed_while_owner_clock_is_behind() -> TestResult {
    // Given: an owner and member enrolled at NOW_MS.
    let owner_dir = TempState::new("clock-behind-ops-owner")?;
    let member_dir = TempState::new("clock-behind-ops-member")?;
    let owner_config = StoreConfig::new(&owner_dir.0);
    let member_config = StoreConfig::new(&member_dir.0);
    let owner_clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS)));
    let member_clock = Arc::new(MutableClock(AtomicI64::new(NOW_MS + 100_000)));
    let owner = Runtime::start_with_clock(
        owner_config.clone(),
        Arc::<MutableClock>::clone(&owner_clock),
    )
    .await?;
    let member = Runtime::start_with_clock(
        member_config.clone(),
        Arc::<MutableClock>::clone(&member_clock),
    )
    .await?;
    let space = owner
        .handle()
        .create_owned_space("clock-ops".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x91; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    let owner_id = owner.handle().status().await?.endpoint_id();
    let member_id = member.handle().status().await?.endpoint_id();

    // Publish the owner's address at NOW_MS + 300_100, then step the owner's clock back to
    // NOW_MS + 200_000 (behind its own address issue time, while the member's NOW_MS + 100_000
    // record stays valid on the owner and the owner's NOW_MS + 300_100 record stays valid on
    // the member).
    owner_clock.0.store(NOW_MS + 300_100, Ordering::Relaxed);
    member_clock.0.store(NOW_MS + 300_100, Ordering::Relaxed);
    owner.handle().publish_address().await?;
    timeout(PROMPT, member.handle().sync_control()).await??;
    owner_clock.0.store(NOW_MS + 200_000, Ordering::Relaxed);
    assert!(owner.handle().publish_address().await.is_err());

    // When / Then (1): inbound control sync on the stepped-back owner answers the member.
    timeout(PROMPT, member.handle().sync_control()).await??;
    assert!(
        member
            .handle()
            .control_sync_status(owner_id)
            .await?
            .into_value()
    );

    // When / Then (2): outbound control sync from the stepped-back owner also succeeds.
    timeout(PROMPT, owner.handle().sync_control()).await??;
    assert!(
        owner
            .handle()
            .control_sync_status(member_id)
            .await?
            .into_value()
    );

    // When / Then (3): authority-side departure succeeds while the owner's clock is behind.
    timeout(
        PROMPT,
        member
            .handle()
            .leave_space(space, RequestId::try_from([0x92; 16].as_slice())?),
    )
    .await??;
    assert!(!member.handle().status().await?.memberships.contains(&space));
    let chain = Repository::open(&owner_config)?
        .load_space_chain(space)?
        .ok_or("owner chain missing")?;
    assert!(
        !chain
            .members()
            .iter()
            .any(|member| member.endpoint_id() == member_id)
    );
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retrying_a_failed_control_completion_emits_memberships_changed_only_once() -> TestResult {
    // Given: two peers sharing a Space; the member will experience one transient completion fault
    // when learning a manifest advance from the owner.
    let owner_dir = TempState::new("control-event-once-owner")?;
    let member_dir = TempState::new("control-event-once-member")?;
    let owner_config = StoreConfig::new(&owner_dir.0);
    let owner = Runtime::start(owner_config.clone()).await?;
    let member = Runtime::start(StoreConfig::new(&member_dir.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("event-once".to_owned())
        .await?;
    let ticket = owner
        .handle()
        .create_enrollment_invite(EnrollmentCreation::new(
            space,
            300_000,
            InviteEntropy::random()?,
        )?)
        .await?;
    member
        .handle()
        .redeem_enrollment(EnrollmentAttempt::new(
            ticket,
            RequestId::try_from([0x93; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    timeout(PROMPT, member.handle().sync_control()).await??;
    let member_id = member.handle().status().await?.endpoint_id();

    // Advance the Space manifest in the owner's Store directly so no unsolicited push races
    // with the member's explicit control sync below.
    let mut repo = Repository::open(&owner_config)?;
    let chain = repo.load_space_chain(space)?.ok_or("owner chain missing")?;
    let mut members = chain.members().to_vec();
    for entry in &mut members {
        if entry.endpoint_id() == member_id {
            *entry = ma2a_core::SpaceMemberV1::new(
                entry.endpoint_id(),
                entry.label().to_owned(),
                entry.capabilities().with(ma2a_core::Capability::FILES_READ),
            )?;
        }
    }
    repo.advance_owned_space(&ma2a_store::OwnedSpaceUpdate::new(
        space,
        u64::try_from(NOW_MS)?,
        ma2a_core::SpaceManifestMembership::new(members, chain.revocations().to_vec()),
    ))?;
    drop(repo);

    let mut events = member.handle().subscribe();
    member.handle().fail_background_once(
        FaultPoint::RelayCandidateLoad,
        RuntimeError::from(StoreError::Io(std::io::ErrorKind::Interrupted.into())),
    );

    // When: the first control sync pulls the manifest and fails completion, then a retry succeeds.
    assert!(
        timeout(PROMPT, member.handle().sync_control())
            .await?
            .is_err()
    );
    timeout(PROMPT, member.handle().sync_control()).await??;

    // Then: `memberships_changed` was emitted only once for that manifest advance.
    let mut count = 0;
    while let Ok(event) = events.try_recv() {
        if event == RuntimeEvent::memberships_changed(event.revision()) {
            count += 1;
        }
    }
    assert_eq!(count, 1, "expected exactly one memberships_changed event");
    timeout(PROMPT, owner.shutdown()).await??;
    timeout(PROMPT, member.shutdown()).await??;
    Ok(())
}
