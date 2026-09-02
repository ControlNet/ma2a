use ma2a_core::{EndpointId, SpaceId};
use ma2a_runtime::api::{
    CapabilityFlags, ClientSnapshotState, Command, CommandResult, ConnectionView, ControlSyncView,
    EchoReplyView, EchoSummaryView, EndpointView, HandshakeAuth, HandshakeState, HandshakeView,
    InteractionCapabilities, ManagementCapabilities, NetworkSnapshotState, ObservedRelayStateView,
    PrivateRelayView, PublicRelayView, ReachabilityView, RelayAddress, RelayCandidateView,
    RelayCapabilities, RuntimeEvent, RuntimeSnapshot, RuntimeStatusView, SnapshotCollections,
    SnapshotHeader, SnapshotState, SpaceView, UiAuthView, decode_command,
};

type FixtureResult<T> = Result<T, Box<dyn std::error::Error>>;

const REQUEST_ID: &str = "000102030405060708090a0b0c0d0e0f";
const ENDPOINT_ID: &str = "5866666666666666666666666666666666666666666666666666666666666666";
const SPACE_ID: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

pub(crate) fn commands() -> FixtureResult<Vec<Command>> {
    command_json()
        .iter()
        .map(|request| decode_command(request.as_bytes()).map_err(Into::into))
        .collect()
}

pub(crate) fn results() -> FixtureResult<Vec<CommandResult>> {
    let endpoint_id = endpoint_id()?;
    let endpoint = EndpointView::new(endpoint_id, "ma2a-runtime", true)?;
    let space = SpaceView::new(space_id()?, "ops", 3)?;
    let control_sync = ControlSyncView::new(vec![endpoint_id])?;
    let ui_auth = UiAuthView::new(true, true, 2);
    let capabilities = CapabilityFlags::new(
        ManagementCapabilities::new(true, true),
        RelayCapabilities::new(true, true),
        InteractionCapabilities::new(true, true),
    );
    let handshake = HandshakeView::new(
        "ma2a-runtime",
        endpoint_id,
        HandshakeState::new(7, HandshakeAuth::new(true, true), capabilities),
    )?;
    let private_relay = PrivateRelayView::new(
        true,
        RelayAddress::new("native_tls", "relay.example", 443)?,
        true,
    );
    let public_relay = PublicRelayView::new(true, Some("https://relay.example".to_owned()), true)?;
    let snapshot = snapshot(SnapshotFixture {
        endpoint,
        space: space.clone(),
        control_sync: control_sync.clone(),
        ui_auth: ui_auth.clone(),
    })?;
    Ok(vec![
        CommandResult::handshake(handshake),
        CommandResult::status(RuntimeStatusView::new(7, true, false)),
        CommandResult::endpoint_info(snapshot_endpoint()?),
        CommandResult::space_created(space.clone()),
        CommandResult::spaces(vec![space.clone()])?,
        CommandResult::space(space.clone()),
        CommandResult::space_invitation_created(space.clone()),
        CommandResult::space_redeemed(space.clone()),
        CommandResult::space_revoked(space),
        CommandResult::control_sync_status(control_sync.clone()),
        CommandResult::control_sync_triggered(control_sync),
        CommandResult::private_relay_configured(private_relay.clone()),
        CommandResult::private_relay_status(private_relay),
        CommandResult::public_relay_configured(public_relay.clone()),
        CommandResult::public_relay_status(public_relay),
        CommandResult::echo(EchoReplyView::new(endpoint_id, "hello")?),
        CommandResult::ui_password_set(ui_auth.clone()),
        CommandResult::ui_password_reset(ui_auth.clone()),
        CommandResult::sessions_revoked(ui_auth),
        CommandResult::ui_opened(ma2a_runtime::api::UiOpenView::new(
            "http://127.0.0.1:12345",
        )?),
        CommandResult::snapshot(snapshot),
        CommandResult::shutting_down(),
    ])
}

pub(crate) fn events() -> FixtureResult<Vec<RuntimeEvent>> {
    let endpoint = endpoint_id()?;
    let space = space_id()?;
    Ok(vec![
        RuntimeEvent::snapshot_invalidated(1),
        RuntimeEvent::endpoint_changed(1, vec![endpoint])?,
        RuntimeEvent::spaces_changed(1, vec![space])?,
        RuntimeEvent::control_sync_changed(1, vec![endpoint])?,
        RuntimeEvent::relay_candidates_changed(1, vec![endpoint])?,
        RuntimeEvent::relay_state_changed(1),
        RuntimeEvent::reachability_changed(1, vec![endpoint])?,
        RuntimeEvent::echo_summary_changed(1, vec![endpoint])?,
        RuntimeEvent::ui_auth_changed(1),
    ])
}

struct SnapshotFixture {
    endpoint: EndpointView,
    space: SpaceView,
    control_sync: ControlSyncView,
    ui_auth: UiAuthView,
}

fn snapshot(fixture: SnapshotFixture) -> FixtureResult<RuntimeSnapshot> {
    let collections = SnapshotCollections::new(
        vec![fixture.space],
        fixture.control_sync,
        vec![ConnectionView::new(
            endpoint_id()?,
            "connected",
            "direct",
            Some(12),
        )],
        vec![RelayCandidateView::new(endpoint_id()?, "private", true)?],
    )?;
    let state = SnapshotState::new(
        NetworkSnapshotState::new(
            ObservedRelayStateView::new(true, false),
            ReachabilityView::new(true, true),
        ),
        ClientSnapshotState::new(EchoSummaryView::new(4, 1), fixture.ui_auth),
    );
    RuntimeSnapshot::new(SnapshotHeader::new(7, fixture.endpoint), collections, state)
        .map_err(Into::into)
}

fn snapshot_endpoint() -> FixtureResult<EndpointView> {
    EndpointView::new(endpoint_id()?, "ma2a-runtime", true).map_err(Into::into)
}

fn endpoint_id() -> FixtureResult<EndpointId> {
    EndpointId::try_from([1_u8; 32].as_slice()).map_err(Into::into)
}

fn space_id() -> FixtureResult<SpaceId> {
    SpaceId::try_from([2_u8; 32].as_slice()).map_err(Into::into)
}

fn command_json() -> Vec<String> {
    vec![
        r#"{"version":1,"operation":"handshake"}"#.to_owned(),
        r#"{"version":1,"operation":"status"}"#.to_owned(),
        r#"{"version":1,"operation":"endpoint_info"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"space_create","request_id":"{REQUEST_ID}","name":"ops"}}"#
        ),
        r#"{"version":1,"operation":"space_list"}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"space_show","space_id":"{SPACE_ID}"}}"#),
        format!(
            r#"{{"version":1,"operation":"space_invite","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","ttl_ms":300000,"output_path":"/tmp/invite.ticket"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"space_redeem","request_id":"{REQUEST_ID}","invitation":"ticket"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"space_revoke","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"control_sync_status","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"control_sync_trigger","request_id":"{REQUEST_ID}","peer_endpoint_id":"{ENDPOINT_ID}"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"private_relay_configure","request_id":"{REQUEST_ID}","mode":"native_tls","listen":"127.0.0.1:443","public_url":"https://relay.example","served_space_ids":["{SPACE_ID}"],"certificate_path":"/tmp/relay.cert.pem","private_key_path":"/tmp/relay.key.pem"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"private_relay_disable","request_id":"{REQUEST_ID}"}}"#
        ),
        r#"{"version":1,"operation":"private_relay_status"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"public_relay_configure","request_id":"{REQUEST_ID}","url":"https://relay.example"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"public_relay_disable","request_id":"{REQUEST_ID}"}}"#
        ),
        r#"{"version":1,"operation":"public_relay_status"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"echo_call","request_id":"{REQUEST_ID}","target_endpoint_id":"{ENDPOINT_ID}","payload":"hello"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"ui_password_set","request_id":"{REQUEST_ID}","password":"correct horse battery staple"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"ui_password_reset","request_id":"{REQUEST_ID}","password":"new correct horse battery staple"}}"#
        ),
        format!(r#"{{"version":1,"operation":"session_revoke_all","request_id":"{REQUEST_ID}"}}"#),
        r#"{"version":1,"operation":"ui_open"}"#.to_owned(),
        r#"{"version":1,"operation":"snapshot_fetch"}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"graceful_shutdown","request_id":"{REQUEST_ID}"}}"#),
    ]
}
