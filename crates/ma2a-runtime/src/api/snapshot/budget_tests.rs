//! Synthetic boundary fixtures measure the final stamped snapshot and detail JSON.
use crate::api::{
    ApiResponse, ClientSnapshotState, CommandResult, ControlSyncView, EchoSummaryView,
    EndpointView, MAX_LOCAL_RESPONSE_BYTES, NetworkSnapshotState, ObservedRelayStateView,
    ReachabilityView, RuntimeSnapshot, SnapshotCollections, SnapshotHeader, SnapshotSpaceView,
    SnapshotState, SpaceChainHead, SpaceDetailsView, SpaceMemberView, UiAuthView, encode_response,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn sixty_four_full_space_summaries_and_one_complete_detail_fit_separate_frames() -> TestResult {
    let local = iroh::SecretKey::generate().public().into();
    let mut spaces = Vec::new();
    for index in 0_u8..64 {
        spaces.push(SnapshotSpaceView::new(
            ma2a_core::SpaceId::derive(&[index]),
            &"\"\\".repeat(32),
            SpaceChainHead::new(u64::MAX, [0xab; 32], 64),
            64,
        )?);
    }
    let first = spaces.first().ok_or("missing Space")?.clone();
    let snapshot = RuntimeSnapshot::new(
        SnapshotHeader::new(u64::MAX, EndpointView::new(local, "ma2a-runtime", true)?),
        SnapshotCollections::new(
            spaces,
            ControlSyncView::new(Vec::new())?,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )?,
        SnapshotState::new(
            NetworkSnapshotState::new(
                ObservedRelayStateView::new(false, false),
                ReachabilityView::new("DegradedNoCommonHome", false, false)?,
            ),
            ClientSnapshotState::new(EchoSummaryView::new(0, 0), UiAuthView::new(true, false, 0)),
        ),
    )?;
    let response = encode_response(&ApiResponse::new(
        None,
        u64::MAX,
        CommandResult::snapshot(snapshot),
    ))?;
    let mut stamped: serde_json::Value = serde_json::from_slice(&response)?;
    stamped.as_object_mut().ok_or("invalid envelope")?.insert(
        "runtime_boot_id".to_owned(),
        serde_json::json!("ab".repeat(16)),
    );
    let size = serde_json::to_vec(&stamped)?.len();
    assert!(size <= MAX_LOCAL_RESPONSE_BYTES);
    let mut ids = (0..64)
        .map(|_| ma2a_core::EndpointId::from(iroh::SecretKey::generate().public()))
        .collect::<Vec<_>>();
    ids.sort();
    let members = ids
        .into_iter()
        .map(|id| SpaceMemberView::new(id, &"\"\\".repeat(32), true, true))
        .collect::<Result<Vec<_>, _>>()?;
    let detail = SpaceDetailsView::new(u64::MAX, first.clone(), members.clone())?;
    let response = encode_response(&ApiResponse::new(
        None,
        u64::MAX,
        CommandResult::space_details(detail),
    ))?;
    assert!(response.len() <= MAX_LOCAL_RESPONSE_BYTES);
    let detail: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(
        detail
            .pointer("/result/payload/members")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(64)
    );
    assert!(SpaceDetailsView::new(1, first, members.into_iter().take(63).collect()).is_err());
    eprintln!(
        "64 full Space summaries: {size} bytes; complete 64-member detail: {} bytes",
        response.len()
    );
    Ok(())
}

#[test]
fn complete_large_snapshot_can_be_encoded_for_bounded_streaming() -> TestResult {
    let local = iroh::SecretKey::generate().public().into();
    let spaces = (0_u16..300)
        .map(|index| {
            SnapshotSpaceView::new(
                ma2a_core::SpaceId::derive(&index.to_be_bytes()),
                &"\"\\".repeat(32),
                SpaceChainHead::new(u64::MAX, [0xff; 32], 64),
                64,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot = RuntimeSnapshot::new(
        SnapshotHeader::new(u64::MAX, EndpointView::new(local, "ma2a-runtime", true)?),
        SnapshotCollections::new(
            spaces,
            ControlSyncView::new(Vec::new())?,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )?,
        SnapshotState::new(
            NetworkSnapshotState::new(
                ObservedRelayStateView::new(false, false),
                ReachabilityView::new("AwaitingIrohHome", false, false)?,
            ),
            ClientSnapshotState::new(EchoSummaryView::new(0, 0), UiAuthView::new(true, false, 0)),
        ),
    )?;
    let encoded = encode_response(&ApiResponse::new(
        None,
        u64::MAX,
        CommandResult::snapshot(snapshot),
    ))?;
    assert!(encoded.len() > MAX_LOCAL_RESPONSE_BYTES);
    let value: serde_json::Value = serde_json::from_slice(&encoded)?;
    assert_eq!(
        value
            .pointer("/result/payload/spaces")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(300)
    );
    Ok(())
}

#[test]
fn space_list_can_represent_all_legal_spaces() -> TestResult {
    let spaces = (0_u16..300)
        .map(|index| {
            crate::api::SpaceView::new(
                ma2a_core::SpaceId::derive(&index.to_be_bytes()),
                &"\"\\".repeat(32),
                64,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = CommandResult::spaces(spaces)?;
    let encoded = encode_response(&ApiResponse::new(None, 7, result))?;
    assert!(encoded.len() > MAX_LOCAL_RESPONSE_BYTES);
    Ok(())
}
