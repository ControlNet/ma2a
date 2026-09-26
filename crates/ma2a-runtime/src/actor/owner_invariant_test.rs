use ma2a_store::{Repository, StoreConfig};

use super::relay_lifecycle_test::TempState;
use crate::{Runtime, error::RuntimeErrorCode};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(start_paused = true)]
async fn owner_revoke_rejection_preserves_runtime_and_durable_state() -> TestResult {
    let state = TempState::new("owner-invariant")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let handle = runtime.handle();
    let space = handle.create_owned_space("Owned".to_owned()).await?;
    let before = handle.status().await?;
    let chain = Repository::open(&config)?.load_space_chain(space)?;
    let revision = Repository::open(&config)?.revision()?;
    let schedules = handle.control_schedules();
    let candidate_attempts = handle.candidate_refresh_attempts();
    let mut events = handle.subscribe();
    for _ in 0..3 {
        let error = handle
            .revoke_owned_space_member(space, before.endpoint_id())
            .await
            .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeErrorCode::SPACE_OWNER_CANNOT_BE_REMOVED
        );
        assert!(
            error
                .to_string()
                .contains("Space owner cannot be removed in Phase 1")
        );
        let after = handle.status().await?;
        assert_eq!(after.revision(), before.revision());
        assert_eq!(after.memberships, before.memberships);
        assert_eq!(after.relay, before.relay);
        assert_eq!(handle.control_schedules(), schedules);
        assert_eq!(handle.candidate_refresh_attempts(), candidate_attempts);
        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ));
        let reopened = Repository::open(&config)?;
        assert_eq!(reopened.revision()?, revision);
        assert_eq!(reopened.load_space_chain(space)?, chain);
    }
    runtime.shutdown().await?;
    assert_eq!(Repository::open(&config)?.load_space_chain(space)?, chain);
    Ok(())
}
