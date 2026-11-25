//! A grant the Store refuses before it signs anything has no effect and releases its ID.

use super::{no_effect_refusals::Served, *};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_grant_past_the_space_history_limit_has_no_effect() -> TestResult {
    // Given: an owned Space whose history holds every ordinary change it may.
    let served = Served::start().await?;
    let handle = served.runtime.handle();
    let space = handle.create_owned_space("history".to_owned()).await?;
    let local = handle.status().await?.endpoint_id();
    // Filled through the Store directly: each change through the Runtime costs far more.
    let mut repository = ma2a_store::Repository::open(&StoreConfig::new(&served.state.0))?;
    let owner = repository
        .load_space_chain(space)?
        .ok_or("no chain")?
        .members()
        .first()
        .ok_or("no owner")?
        .clone();
    for generation in 1..=239_u64 {
        let service = crate::store::SERVICE_CAPABILITIES[0];
        let capabilities = if generation % 2 == 1 {
            owner.capabilities().with(service)
        } else {
            owner.capabilities().without(service)
        };
        let member = ma2a_core::SpaceMemberV1::new(local, owner.label().to_owned(), capabilities)?;
        repository.advance_owned_space(&ma2a_store::OwnedSpaceUpdate::new(
            space,
            2_000_000_000_000 + generation,
            ma2a_core::SpaceManifestMembership::new(vec![member], vec![]),
        ))?;
    }
    let revision = repository.revision()?;

    // When / Then: one more change is refused as a conflict, with no effect.
    served
        .refused(
            &serde_json::json!({
                "version": 1, "operation": "space_grant", "request_id": "66".repeat(16),
                "space_id": api::encode_hex(space.as_bytes()),
                "peer_endpoint_id": api::encode_hex(local.as_bytes()),
                "files_read": false, "files_write": false, "exec": false, "shell": false,
                "proxy": false, "process": false, "agent": false, "service": false,
            }),
            ProtocolError::CONFLICT,
        )
        .await?;
    assert_eq!(repository.revision()?, revision);
    served.stop().await
}
