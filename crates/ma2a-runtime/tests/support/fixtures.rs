use ma2a_core::service::{
    AgentEvent, AgentEventBody, AgentId, AgentMessage, AgentTaskDetails, AgentTaskId,
    AgentTaskReason, AgentTaskState, AgentTaskSummary, ArtifactRef, MessagePart, MessageRole,
};
use ma2a_core::{
    EndpointId, RequestId, SpaceId,
    service::{
        DEFAULT_LOG_LIMIT, LogWindow, ProcessId, ProcessReason, ProcessSpec, ProcessStart,
        ProcessState, ProxyHostPattern, ProxyRule, RootAccess, ServiceError, Termination,
    },
};
use ma2a_runtime::api::AgentView;
use ma2a_runtime::api::{
    CapabilityFlags, ClientSnapshotState, Command, CommandResult, ConnectionObservationView,
    ConnectionView, ControlRoundView, ControlSyncView, EchoReplyView, EchoSummaryView,
    EndpointView, HandshakeAuth, HandshakeState, HandshakeView, InteractionCapabilities,
    ManagementCapabilities, NetworkSnapshotState, ObservedRelayStateView,
    PrivateRelayCandidateView, PrivateRelayView, PublicRelayFallbackView, PublicRelayView,
    ReachabilityView, RelayAddress, RelayCapabilities, RuntimeEvent, RuntimeSnapshot,
    RuntimeStatusView, SnapshotCollections, SnapshotHeader, SnapshotSpaceView, SnapshotState,
    SpaceChainHead, SpaceMemberView, SpaceView, UiAuthView, decode_command,
};
use ma2a_runtime::api::{
    OperationEndView, OperationIdentity, OperationRecordView, OperationsView, ProcessDetailsView,
    ProcessView, ServiceExposureView,
};
use ma2a_store::{AgentDefinition, AgentInstanceRecord, AgentRecord, ProcessRecord, ProcessTimes};

type FixtureResult<T> = Result<T, Box<dyn std::error::Error>>;

const REQUEST_ID: &str = "000102030405060708090a0b0c0d0e0f";
const ENDPOINT_ID: &str = "5866666666666666666666666666666666666666666666666666666666666666";
const SPACE_ID: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const PROCESS_ID: &str = "f0e1d2c3b4a5968778695a4b3c2d1e0f";

pub(crate) fn commands() -> FixtureResult<Vec<Command>> {
    command_json()
        .iter()
        .chain(agent_requests().iter())
        .map(|request| decode_command(request.as_bytes()).map_err(Into::into))
        .collect()
}

pub(crate) fn results() -> FixtureResult<Vec<CommandResult>> {
    let endpoint_id = endpoint_id()?;
    let endpoint = EndpointView::new(endpoint_id, "ma2a-runtime", true)?;
    let space = SpaceView::new(space_id()?, "ops", 3)?;
    let snapshot_space =
        SnapshotSpaceView::new(space_id()?, "ops", SpaceChainHead::new(4, [0x5a; 32], 1), 1)?;
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
        space: snapshot_space.clone(),
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
        CommandResult::space_left(ma2a_runtime::api::SpaceIdentityView::new(
            space_id()?,
            "ops",
        )?),
        CommandResult::control_sync_status(control_sync.clone()),
        CommandResult::control_sync_triggered(control_sync),
        CommandResult::private_relay_configured(private_relay.clone()),
        CommandResult::private_relay_status(private_relay),
        CommandResult::public_relay_configured(public_relay.clone()),
        CommandResult::public_relay_status(public_relay),
        CommandResult::echo(EchoReplyView::new(endpoint_id, "hello", 34)?),
        CommandResult::ui_password_set(ui_auth.clone()),
        CommandResult::ui_password_reset(ui_auth.clone()),
        CommandResult::sessions_revoked(ui_auth.clone()),
        CommandResult::ui_initialized(ui_auth),
        CommandResult::ui_status(ma2a_runtime::api::UiStatusView::new(Some(
            "127.0.0.1:12345".parse()?,
        ))),
        CommandResult::snapshot(snapshot),
        CommandResult::space_details(ma2a_runtime::api::SpaceDetailsView::new(
            7,
            snapshot_space,
            vec![SpaceMemberView::new(endpoint_id, "operator", true, false)?],
        )?),
        CommandResult::snapshot_stamp(ma2a_runtime::api::SnapshotStampView::new(7, [0x51; 16])),
        CommandResult::shutting_down(),
        CommandResult::space_granted(
            SpaceMemberView::new(endpoint_id, "operator", true, false)?
                .with_services([true, false, true, false, false, true, false]),
        ),
        CommandResult::service_exposed(ServiceExposureView::files_root(
            space_id()?,
            ("docs", "/srv/docs"),
            RootAccess::ReadWrite,
        )?),
        CommandResult::service_unexposed(ServiceExposureView::exec(space_id()?)),
        CommandResult::service_exposures(vec![
            ServiceExposureView::shell(space_id()?),
            ServiceExposureView::proxy(
                space_id()?,
                ProxyRule::new(ProxyHostPattern::Any, (8000, 8080))?,
            ),
        ]),
        CommandResult::operations(operations()?),
        CommandResult::stream_opened(),
        CommandResult::processes(vec![process(false)?], Some(process_id()?))?,
        CommandResult::process_details(ProcessDetailsView::new(
            process(false)?,
            (Some(process_start()?), LogWindow::new(1_024, 4_096)),
        )),
        CommandResult::process_terminated(process(false)?),
        CommandResult::process_deleted(process(true)?),
    ]
    .into_iter()
    .chain(agent_results()?)
    .collect())
}

/// One started Agent with a ready instance, and one completed task of it.
fn agent_results() -> FixtureResult<Vec<CommandResult>> {
    let agent = AgentId::new("reviewer")?;
    let start = ProcessStart::new(
        ProcessSpec::new(
            Some("/usr/bin/reviewer".to_owned()),
            vec!["--serve".to_owned()],
            None,
        )?
        .with_env("MODE".to_owned(), "strict".to_owned())?,
        true,
        DEFAULT_LOG_LIMIT,
    )?;
    let record = AgentRecord {
        definition: AgentDefinition {
            id: agent.clone(),
            description: "reviews".to_owned(),
            start: start.to_bytes()?,
            tools: 1 | 32,
            max_tasks: 2,
        },
        enabled: true,
        generation: 1,
        times: (1_000, 2_000),
        instance: Some(AgentInstanceRecord {
            generation: 1,
            process_id: Some(process_id()?),
            ready: true,
            cursor: 0,
            next_seq: 1,
            acked_seq: 0,
            started_ms: 2_000,
            ended_ms: None,
        }),
    };
    let view = AgentView::new(record, 1);
    let mut summary = AgentTaskSummary::new(
        (AgentTaskId::parse(PROCESS_ID)?, agent),
        (
            EndpointId::try_from(decode(ENDPOINT_ID)?.as_slice())?,
            RequestId::try_from(decode(REQUEST_ID)?.as_slice())?,
        ),
        3_000,
    );
    summary.state = AgentTaskState::Completed;
    summary.times = (3_000, Some(3_100), Some(4_000));
    summary.last_seq = 4;
    let input = AgentMessage::new(
        MessageRole::User,
        vec![
            MessagePart::Text("review".to_owned()),
            MessagePart::Data("[1,2]".to_owned()),
        ],
    )?;
    let artifact = ArtifactRef::new(("report.txt", "text/plain"), (12, [0x42; 32]))?;
    let mut details = AgentTaskDetails::new(summary.clone(), input.clone());
    details.result = Some(AgentMessage::text(MessageRole::Agent, "done")?);
    details.artifacts = vec![artifact.clone()];
    let events = vec![
        AgentEvent::new(
            1,
            3_000,
            AgentEventBody::State(AgentTaskState::Queued, None),
        ),
        AgentEvent::new(2, 3_050, AgentEventBody::Message(input)),
        AgentEvent::new(3, 3_500, AgentEventBody::progress("halfway")?),
        AgentEvent::new(4, 3_900, AgentEventBody::Artifact(artifact)),
        AgentEvent::new(
            5,
            4_000,
            AgentEventBody::State(AgentTaskState::Cancelled, Some(AgentTaskReason::Requested)),
        ),
    ];
    Ok(vec![
        CommandResult::agents(vec![view.clone()]),
        CommandResult::agent_defined(&view, true),
        CommandResult::agent_undefined("reviewer"),
        CommandResult::agent_started(&view),
        CommandResult::agent_stopped(&view),
        CommandResult::agent_tasks(vec![summary.clone()]),
        CommandResult::agent_task(&details),
        CommandResult::agent_task_events(events),
        CommandResult::agent_task_submitted(&summary, false),
        CommandResult::agent_task_sent(&summary),
        CommandResult::agent_task_cancelled(&summary),
    ])
}

fn decode(hex: &str) -> FixtureResult<Vec<u8>> {
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            Ok(u8::from_str_radix(
                hex.get(index..index + 2).unwrap_or("zz"),
                16,
            )?)
        })
        .collect()
}

fn process_id() -> FixtureResult<ProcessId> {
    Ok(ProcessId::parse(PROCESS_ID)?)
}

fn process_start() -> FixtureResult<ProcessStart> {
    let spec = ProcessSpec::new(
        Some("/usr/bin/tail".to_owned()),
        vec!["-f".to_owned(), "/var/log/app.log".to_owned()],
        Some("/srv/app".to_owned()),
    )?
    .with_env("APP_MODE".to_owned(), "fixture".to_owned())?;
    Ok(ProcessStart::new(spec, true, DEFAULT_LOG_LIMIT)?
        .with_name("tail".to_owned())?
        .with_label("team".to_owned(), "ops".to_owned())?)
}

/// A Process that ran and was terminated; `archived` also drops its start.
fn process(archived: bool) -> FixtureResult<ProcessView> {
    Ok(ProcessView::new(ProcessRecord {
        id: process_id()?,
        creator: endpoint_id()?,
        request_id: RequestId::try_from([0x0f_u8; 16].as_slice())?,
        fingerprint: [0x2a; 32],
        start: if archived {
            None
        } else {
            Some(process_start()?.to_bytes()?)
        },
        name: Some("tail".to_owned()),
        state: ProcessState::Terminated,
        reason: Some(ProcessReason::Requested),
        termination: Some(Termination::Signaled(15)),
        secret: None,
        times: ProcessTimes {
            created_ms: 1_000,
            started_ms: Some(1_010),
            ended_ms: Some(2_000),
            archived_ms: archived.then_some(3_000),
        },
    }))
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
    space: SnapshotSpaceView,
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
            vec![ConnectionObservationView::new(
                1_700_000_000_000,
                "direct",
                Some(12),
                "none",
            )],
        )?],
        vec![PrivateRelayCandidateView::new(
            endpoint_id()?,
            "https://relay.example",
            vec![space_id()?],
            true,
        )?],
        vec![PublicRelayFallbackView::new(
            "https://public.example",
            true,
            false,
        )?],
        vec![ControlRoundView::new(1_700_000_000_000, 1, "succeeded")?],
    )?;
    let state = SnapshotState::new(
        NetworkSnapshotState::new(
            ObservedRelayStateView::new(true, false),
            ReachabilityView::new("IrohHomeConnected", true, true)?,
        ),
        ClientSnapshotState::new(EchoSummaryView::new(4, 1), fixture.ui_auth),
    );
    RuntimeSnapshot::new(SnapshotHeader::new(7, fixture.endpoint), collections, state)
        .map_err(Into::into)
}

fn snapshot_endpoint() -> FixtureResult<EndpointView> {
    EndpointView::new(endpoint_id()?, "ma2a-runtime", true).map_err(Into::into)
}

fn operations() -> FixtureResult<OperationsView> {
    let request_id = ma2a_core::RequestId::try_from([3_u8; 16].as_slice())?;
    let upload = OperationIdentity::new(
        request_id,
        endpoint_id()?,
        (ma2a_core::ServiceKind::Transfer, "upload", false),
    );
    let exec = OperationIdentity::new(
        request_id,
        endpoint_id()?,
        (ma2a_core::ServiceKind::Exec, "exec", true),
    );
    Ok(OperationsView::new(
        vec![(upload, 1_200)],
        vec![
            OperationRecordView::new(exec, OperationEndView::completed(), (5, 1_024))
                .with_duration_ms(40),
            OperationRecordView::new(
                upload,
                OperationEndView::failed(ServiceError::IntegrityMismatch),
                (4_096, 0),
            ),
        ],
    ))
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
            r#"{{"version":1,"operation":"space_leave","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}"}}"#
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
        format!(
            r#"{{"version":1,"operation":"ui_init","request_id":"{REQUEST_ID}","password":"correct horse battery staple"}}"#
        ),
        r#"{"version":1,"operation":"ui_start","host":"127.0.0.1","port":0}"#.to_owned(),
        r#"{"version":1,"operation":"ui_stop"}"#.to_owned(),
        r#"{"version":1,"operation":"ui_status"}"#.to_owned(),
        r#"{"version":1,"operation":"snapshot_fetch"}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"space_details_fetch","space_id":"{SPACE_ID}"}}"#),
        r#"{"version":1,"operation":"snapshot_stamp"}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"graceful_shutdown","request_id":"{REQUEST_ID}"}}"#),
        format!(
            r#"{{"version":1,"operation":"space_grant","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","peer_endpoint_id":"{ENDPOINT_ID}","files_read":true,"files_write":false,"exec":true,"shell":false,"proxy":true,"process":false,"agent":true}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"service_expose","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","kind":"files_root","root":"docs","path":"/srv/docs","access":"read_only","host":null,"port_start":null,"port_end":null,"agent":null}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"service_unexpose","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}","kind":"proxy","root":null,"host":"*","port_start":8000,"port_end":8080,"agent":null}}"#
        ),
        r#"{"version":1,"operation":"service_exposure_list"}"#.to_owned(),
        r#"{"version":1,"operation":"operations_status"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"stream_open","request_id":"{REQUEST_ID}","target_endpoint_id":"{ENDPOINT_ID}","service":"transfer"}}"#
        ),
        r#"{"version":1,"operation":"process_list","after":null}"#.to_owned(),
        format!(r#"{{"version":1,"operation":"process_inspect","process_id":"{PROCESS_ID}"}}"#),
        format!(
            r#"{{"version":1,"operation":"process_terminate","request_id":"{REQUEST_ID}","process_id":"{PROCESS_ID}","grace_ms":5000}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"process_delete","request_id":"{REQUEST_ID}","process_id":"{PROCESS_ID}"}}"#
        ),
    ]
}

/// Every `agent_*` command, in inventory order.
fn agent_requests() -> Vec<String> {
    vec![
        format!(
            r#"{{"version":1,"operation":"agent_define","request_id":"{REQUEST_ID}","agent":"reviewer","description":"reviews","program":"/usr/bin/reviewer","args":["--serve"],"cwd":"/srv/reviewer","env":[{{"name":"MODE","value":"strict"}}],"tools":["files","agent"],"max_tasks":2}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"agent_undefine","request_id":"{REQUEST_ID}","agent":"reviewer"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"agent_start","request_id":"{REQUEST_ID}","agent":"reviewer"}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"agent_stop","request_id":"{REQUEST_ID}","agent":"reviewer"}}"#
        ),
        r#"{"version":1,"operation":"agent_list","after":"planner"}"#.to_owned(),
        format!(
            r#"{{"version":1,"operation":"agent_task_list","agent":null,"after":"{PROCESS_ID}"}}"#
        ),
        format!(r#"{{"version":1,"operation":"agent_task_inspect","task_id":"{PROCESS_ID}"}}"#),
        format!(
            r#"{{"version":1,"operation":"agent_task_events","task_id":"{PROCESS_ID}","after":0}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"agent_task_submit","request_id":"{REQUEST_ID}","agent":"reviewer","message":{{"role":"user","parts":[{{"kind":"text","content":"review"}},{{"kind":"data","content":"[1,2]"}}]}}}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"agent_task_send","request_id":"{REQUEST_ID}","task_id":"{PROCESS_ID}","message":{{"role":"user","parts":[{{"kind":"text","content":"more"}}]}}}}"#
        ),
        format!(
            r#"{{"version":1,"operation":"agent_task_cancel","request_id":"{REQUEST_ID}","task_id":"{PROCESS_ID}"}}"#
        ),
    ]
}
