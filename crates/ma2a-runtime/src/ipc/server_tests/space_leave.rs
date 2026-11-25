//! A departure waiting on its Space authority holds up neither other local mutations nor a
//! second departure's prompt refusal.
//!
//! The exchange runs on real Iroh I/O, so this uses real time.
use std::time::Duration;

use ma2a_core::{InviteEntropy, RequestId};
use tokio::time::timeout;

use super::*;
use crate::{EnrollmentAttempt, EnrollmentCreation};

/// Far below the thirty-second exchange bound, far above one actor turn.
const PROMPT: Duration = Duration::from_secs(5);

fn error_name(reply: &[u8]) -> TestResult<Option<String>> {
    let value: serde_json::Value = serde_json::from_slice(reply)?;
    Ok(value
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_second_leave_is_refused_while_the_first_waits_on_the_authority() -> TestResult {
    // Given: a member, served over IPC, whose Space authority has stopped answering.
    let owner_state = TempState::new_named("leave-owner")?;
    let member_state = TempState::new_named("leave-member")?;
    let owner = Runtime::start(StoreConfig::new(&owner_state.0)).await?;
    let member = Runtime::start(StoreConfig::new(&member_state.0)).await?;
    let space = owner
        .handle()
        .create_owned_space("leave".to_owned())
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
            RequestId::try_from([0x81; 16].as_slice())?,
            "member".to_owned(),
        ))
        .await?;
    owner.shutdown().await?;
    let paths = IpcPaths::new(&member_state.0)?;
    let control = CurrentUserRuntime::open_at(
        &member_state.0,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await?;
    let server = LiveServer::spawn(
        LocalApiServer::bind_for_launch(paths.clone(), member.handle(), control, None).await?,
    );
    let space_hex = api::encode_hex(space.as_bytes());
    let leave = |request: &str| {
        api::decode_command(
            format!(
                r#"{{"version":1,"operation":"space_leave","request_id":"{request}","space_id":"{space_hex}"}}"#
            )
            .as_bytes(),
        )
    };
    let first = leave("82828282828282828282828282828282")?;
    let second = leave("83838383838383838383838383838383")?;
    let create = api::decode_command(
        br#"{"version":1,"operation":"space_create","request_id":"84848484848484848484848484848484","name":"meanwhile"}"#,
    )?;

    // When: one client leaves, and others ask while that departure is still pending.
    let client = LocalApiClient::new(paths.clone());
    let leaving = tokio::spawn(async move { client.call(&first).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let client = LocalApiClient::new(paths);
    let refused = timeout(PROMPT, client.call(&second)).await??;
    let created = timeout(PROMPT, client.call(&create)).await??;

    // Then: the second leave is refused at once and leaves no replay entry behind, and the
    // unrelated mutation completes, all before the first departure is answered.
    assert!(
        !leaving.is_finished(),
        "the first leave must still be pending"
    );
    assert_eq!(
        error_name(&refused)?.as_deref(),
        Some(ProtocolError::CONFLICT.name())
    );
    let second_id = second.request_id().ok_or("request id missing")?;
    assert_eq!(member.handle().mutation_replay(second_id).await?, None);
    assert_eq!(error_name(&created)?, None);

    // Stopping the Runtime ends the pending exchange instead of waiting it out.
    timeout(PROMPT, member.shutdown()).await??;
    let abandoned = timeout(PROMPT, leaving).await??;
    assert!(abandoned.is_err() || error_name(&abandoned?)?.is_some());
    drop(server);
    Ok(())
}
