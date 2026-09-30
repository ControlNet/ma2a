//! Two real Runtimes over real Iroh: the caller's stream plane, the target's host, and the
//! replay, conflict, and loss contract between them.

use ma2a_core::{
    EndpointId, InviteEntropy, RequestId, ServiceKind,
    service::{
        FilesRequest, FilesResult, FrameKind, OpenBody, OpenHeader, RemotePath, ServiceError,
    },
};
use ma2a_services::{Frame, FrameReader, FrameWriter};
use ma2a_store::{FileRootExposure, LocalExposure};

use super::*;
use crate::{
    EnrollmentAttempt, EnrollmentCreation,
    ipc::{LocalStream, StreamUpgrade},
};

struct Pair {
    target: Runtime,
    caller: Runtime,
    live_server: LiveServer,
    client: LocalApiClient,
    hooks: super::super::mutation_test_hooks::MutationTestHooks,
    target_id: EndpointId,
    root: TempState,
    _states: (TempState, TempState),
}

impl Pair {
    /// Joins a caller to the target's Space, grants it Files, and exposes one root.
    async fn start(label: &str) -> TestResult<Self> {
        let target_state = TempState::new_named(&format!("{label}-target"))?;
        let caller_state = TempState::new_named(&format!("{label}-caller"))?;
        let root = TempState::new_named(&format!("{label}-root"))?;
        let target = Runtime::start(StoreConfig::new(&target_state.0)).await?;
        let caller = Runtime::start(StoreConfig::new(&caller_state.0)).await?;
        let space = target.handle().create_owned_space("svc".to_owned()).await?;
        let ticket = target
            .handle()
            .create_enrollment_invite(EnrollmentCreation::new(
                space,
                300_000,
                InviteEntropy::from_bytes([0x31; 16], [0x41; 32]),
            )?)
            .await?;
        caller
            .handle()
            .redeem_enrollment(EnrollmentAttempt::new(
                ticket,
                RequestId::try_from([0x51; 16].as_slice())?,
                "caller".to_owned(),
            ))
            .await?;
        let caller_id = caller.handle().status().await?.endpoint_id();
        let granted = target
            .handle()
            .grant_owned_space_member((space, caller_id), [true, true, false, false, false])
            .await?;
        assert!(granted.is_ok());
        let canonical = std::fs::canonicalize(&root.0)?
            .into_os_string()
            .into_string()
            .map_err(|_| "path")?;
        let exposure = LocalExposure::FilesRoot(FileRootExposure::new(
            ma2a_core::service::RootName::new("docs")?,
            canonical,
            ma2a_core::service::RootAccess::ReadWrite,
        ));
        target.handle().expose_service(space, exposure).await??;
        let control = CurrentUserRuntime::open_at(
            &caller_state.0,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        let paths = IpcPaths::new(&caller_state.0)?;
        let server =
            LocalApiServer::bind_for_launch(paths.clone(), caller.handle(), control, None).await?;
        let hooks = server.mutation_hooks.clone();
        Ok(Self {
            target_id: target.handle().status().await?.endpoint_id(),
            target,
            caller,
            live_server: LiveServer::spawn(server),
            client: LocalApiClient::new(paths),
            hooks,
            root,
            _states: (target_state, caller_state),
        })
    }

    /// Runs one Files request through the stream plane and returns its terminal frame.
    async fn files(&self, request_id: RequestId, request: FilesRequest) -> TestResult<Frame> {
        let upgrade = api::decode_command(&serde_json::to_vec(&serde_json::json!({
            "version": 1, "operation": "stream_open",
            "request_id": api::encode_hex(request_id.as_bytes()),
            "target_endpoint_id": api::encode_hex(self.target_id.as_bytes()),
            "service": ServiceKind::Files.name(),
        }))?)?;
        let StreamUpgrade::Opened(stream) = self.client.open_stream(&upgrade).await? else {
            return Err("stream_open refused".into());
        };
        let (mut reader, mut writer) = halves(stream);
        let open =
            OpenHeader::new(request_id, self.target_id, OpenBody::Files(request)).encode()?;
        writer.frame(FrameKind::Open, &open).await?;
        writer.flush().await?;
        Ok(reader.expect().await?)
    }

    fn target_operations(&self) -> usize {
        let snapshot = self.target.handle().operations().snapshot();
        snapshot.recent.len() + snapshot.active.len()
    }

    async fn stop(self) -> TestResult {
        assert_eq!(self.live_server.cancel().await?, ServerExit::Cancelled);
        self.caller.shutdown().await?;
        self.target.shutdown().await?;
        Ok(())
    }
}

type Halves = (
    FrameReader<tokio::io::ReadHalf<LocalStream>>,
    FrameWriter<tokio::io::WriteHalf<LocalStream>>,
);

fn halves(stream: LocalStream) -> Halves {
    let (read, write) = tokio::io::split(stream);
    (FrameReader::new(read), FrameWriter::new(write))
}

fn id(byte: u8) -> TestResult<RequestId> {
    Ok(RequestId::try_from([byte; 16].as_slice())?)
}

fn mkdir(path: &str) -> TestResult<FilesRequest> {
    Ok(FilesRequest::Mkdir(RemotePath::parse(path)?, false))
}

fn outcome(frame: &Frame) -> TestResult<Result<FilesResult, ServiceError>> {
    assert_eq!(frame.kind(), FrameKind::Outcome);
    Ok(FilesResult::decode_outcome(frame.payload())?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_completed_mutation_replays_its_outcome_and_a_changed_open_conflicts() -> TestResult {
    let pair = Pair::start("stream-replay").await?;
    let first = pair.files(id(0x61)?, mkdir("docs:/made")?).await?;
    assert_eq!(outcome(&first)?, Ok(FilesResult::Done));
    assert!(pair.root.0.join("made").is_dir());
    let served = pair.target_operations();

    let replayed = pair.files(id(0x61)?, mkdir("docs:/made")?).await?;
    assert_eq!(replayed.payload(), first.payload());
    assert_eq!(
        pair.target_operations(),
        served,
        "a replay never reaches the target"
    );

    let conflict = pair.files(id(0x61)?, mkdir("docs:/other")?).await?;
    assert_eq!(conflict.rejection(), Ok(ServiceError::Conflict));
    assert!(!pair.root.0.join("other").exists());

    let again = pair.files(id(0x62)?, mkdir("docs:/made")?).await?;
    // An existing directory is refused before any effect, so it is a reject, not a failure.
    assert_eq!(again.rejection(), Ok(ServiceError::AlreadyExists));
    pair.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stream_lost_after_open_is_settled_as_unknown_and_never_retried() -> TestResult {
    let pair = Pair::start("stream-loss").await?;
    pair.hooks
        .lose_stream_after_open
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let lost = pair.files(id(0x71)?, mkdir("docs:/maybe")?).await?;
    assert_eq!(outcome(&lost)?, Err(ServiceError::OutcomeUnknown));
    let replayed = pair.files(id(0x71)?, mkdir("docs:/maybe")?).await?;
    assert_eq!(replayed.payload(), lost.payload());
    let caller_ops = pair.caller.handle().operations().snapshot();
    assert!(caller_ops.active.is_empty());
    pair.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_mutation_releases_its_request_and_reads_never_reserve() -> TestResult {
    let pair = Pair::start("stream-release").await?;
    let outside = pair.files(id(0x81)?, mkdir("other:/x")?).await?;
    assert_eq!(outside.rejection(), Ok(ServiceError::Unauthorized));
    // The reject proved nothing happened, so the same request may run once it is valid.
    let retried = pair.files(id(0x81)?, mkdir("docs:/x")?).await?;
    assert_eq!(outcome(&retried)?, Ok(FilesResult::Done));

    let stat = FilesRequest::Stat(RemotePath::parse("docs:/x")?);
    let first = pair.files(id(0x82)?, stat.clone()).await?;
    let second = pair
        .files(id(0x82)?, FilesRequest::Stat(RemotePath::parse("docs:/")?))
        .await?;
    assert!(matches!(outcome(&first)?, Ok(FilesResult::Metadata(_))));
    assert!(matches!(outcome(&second)?, Ok(FilesResult::Metadata(_))));
    pair.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_targeted_or_mismatched_open_is_refused() -> TestResult {
    let pair = Pair::start("stream-refusal").await?;
    let caller_id = pair.caller.handle().status().await?.endpoint_id();
    let upgrade = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1, "operation": "stream_open",
        "request_id": api::encode_hex(id(0x91)?.as_bytes()),
        "target_endpoint_id": api::encode_hex(caller_id.as_bytes()),
        "service": "files",
    }))?)?;
    let StreamUpgrade::Refused(response) = pair.client.open_stream(&upgrade).await? else {
        return Err("self-targeted upgrade was accepted".into());
    };
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(
        response.get("error"),
        Some(&serde_json::json!("invalid_input"))
    );

    let upgrade = api::decode_command(&serde_json::to_vec(&serde_json::json!({
        "version": 1, "operation": "stream_open",
        "request_id": api::encode_hex(id(0x92)?.as_bytes()),
        "target_endpoint_id": api::encode_hex(pair.target_id.as_bytes()),
        "service": "exec",
    }))?)?;
    let StreamUpgrade::Opened(stream) = pair.client.open_stream(&upgrade).await? else {
        return Err("upgrade refused".into());
    };
    let (mut reader, mut writer) = halves(stream);
    let open = OpenHeader::new(
        id(0x92)?,
        pair.target_id,
        OpenBody::Files(mkdir("docs:/y")?),
    )
    .encode()?;
    writer.frame(FrameKind::Open, &open).await?;
    writer.flush().await?;
    assert_eq!(
        reader.expect().await?.rejection(),
        Ok(ServiceError::InvalidInput)
    );
    pair.stop().await
}
