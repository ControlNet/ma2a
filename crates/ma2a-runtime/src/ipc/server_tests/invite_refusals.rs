//! Invitation requests refused before they run leave nothing behind and release their ID.

use super::{
    no_effect_refusals::{Served, invitations},
    *,
};

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invitation_with_a_bad_lifetime_never_touches_its_directory() -> TestResult {
    // Given: an output directory whose modification time is known.
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("lifetime".to_owned())
        .await?;
    let directory = served.state.0.join("untouched");
    std::fs::create_dir(&directory)?;
    let known = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
    std::fs::File::open(&directory)?.set_modified(known)?;
    let request = serde_json::json!({
        "version": 1, "operation": "space_invite", "request_id": "63".repeat(16),
        "space_id": api::encode_hex(space.as_bytes()), "ttl_ms": 600_000,
        "output_path": directory.join("ticket").to_str().ok_or("invalid path")?,
    });

    // When
    served
        .refused(&request, ProtocolError::INVALID_INPUT)
        .await?;

    // Then: nothing was created in it, not even for a moment.
    assert_eq!(std::fs::metadata(&directory)?.modified()?, known);
    served.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_invitation_for_an_unknown_space_can_be_corrected_under_its_request_id() -> TestResult {
    // Given: an invitation request naming a Space this Runtime does not hold.
    let served = Served::start().await?;
    let space = served
        .runtime
        .handle()
        .create_owned_space("known".to_owned())
        .await?;
    let output = served.state.0.join("ticket");
    let output = output.to_str().ok_or("invalid path")?;
    let invite = |space_id: String| {
        serde_json::json!({
            "version": 1, "operation": "space_invite", "request_id": "64".repeat(16),
            "space_id": space_id, "ttl_ms": 60_000, "output_path": output,
        })
    };
    served
        .refused(&invite("65".repeat(32)), ProtocolError::NOT_FOUND)
        .await?;
    // Its staging is gone by the time the refusal is answered.
    let staged = std::fs::read_dir(&served.state.0)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".ma2a-invite-stage-")
        })
        .count();
    assert_eq!(staged, 0);

    // When: the same request is sent again for the right Space.
    let corrected = invite(api::encode_hex(space.as_bytes()));
    let command = api::decode_command(&serde_json::to_vec(&corrected)?)?;
    let value: serde_json::Value = serde_json::from_slice(&served.client.call(&command).await?)?;

    // Then: it runs, once.
    assert_eq!(value.get("error"), None, "{value}");
    assert_eq!(invitations(&served)?, 1);
    served.stop().await
}
