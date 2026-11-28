//! The Process manager against a real Store, real sockets, and in-process supervisors.
#![cfg(unix)]

mod authorization;
mod driver;
mod lifecycle;
mod local_limit;
mod logs;
mod recovery;
mod relay;
mod startup_recovery;
mod supervisor_loss;
mod terminating;
mod watch;

use std::{path::PathBuf, sync::Arc, time::Duration};

use ma2a_core::{
    EndpointId, MemberCapabilities, RequestId, SpaceId, SpaceManifestMembership, SpaceMemberV1,
    SpacePolicyV1,
    service::{ProcessId, ProcessSpec, ProcessStart, ProcessState},
};
use ma2a_store::{
    LocalExposure, OwnedSpaceUpdate, ProcessCreation, ProcessRecord, SpaceCreation, StoreConfig,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{ProcessHosting, ProcessManager};
use crate::{
    clock::SystemClock,
    store::{OwnedMemberGrant, STORE_CAPACITY, StoreBackend, StoreClient},
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const MIB: u32 = 1_048_576;

/// A private state directory removed on drop.
struct TempState(PathBuf);

impl TempState {
    /// Creates a directory with a short name: a supervisor's socket lives several levels below
    /// it, and a Unix socket path holds at most 103 bytes. `label` names it in diagnostics only.
    fn new(label: &str) -> TestResult<Self> {
        use std::os::unix::fs::PermissionsExt as _;
        let mut nonce = [0_u8; 4];
        getrandom::fill(&mut nonce)?;
        let nonce = u32::from_le_bytes(nonce);
        let path = std::env::temp_dir().join(format!("ma2a-{nonce:08x}"));
        eprintln!("{label}: state directory {}", path.display());
        std::fs::create_dir(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    state: TempState,
    store: StoreClient,
    local: EndpointId,
    creator: EndpointId,
    space: SpaceId,
}

impl Harness {
    async fn new(label: &str) -> TestResult<Self> {
        let state = TempState::new(label)?;
        let backend = StoreBackend::open(&StoreConfig::new(&state.0))?;
        let (sender, receiver) = mpsc::channel(STORE_CAPACITY);
        tokio::task::spawn_blocking(move || backend.run(receiver));
        let store = StoreClient::new(sender);
        let local = store.initialize().await?.endpoint_id;
        let creator: EndpointId = iroh::SecretKey::generate().public().into();
        let owner = SpaceMemberV1::new(
            local,
            "owner".to_owned(),
            MemberCapabilities::new(true, false),
        )?;
        let peer = SpaceMemberV1::new(
            creator,
            "creator".to_owned(),
            MemberCapabilities::new(true, false),
        )?;
        let space = store
            .create_owned_space(SpaceCreation::new(
                1_000,
                owner.clone(),
                SpacePolicyV1::phase_one_default(),
            ))
            .await?
            .space_id();
        store
            .processes(move |repository| -> Result<(), ma2a_store::StoreError> {
                let mut members = vec![owner, peer];
                members.sort_by_key(SpaceMemberV1::endpoint_id);
                repository.advance_owned_space(&OwnedSpaceUpdate::new(
                    space,
                    2_000,
                    SpaceManifestMembership::new(members, Vec::new()),
                ))?;
                repository.expose_service(space, &LocalExposure::Process)?;
                Ok(())
            })
            .await??;
        store
            .grant_owned_space_member(OwnedMemberGrant {
                space_id: space,
                endpoint_id: creator,
                services: [true; 8],
                issued_at_ms: 3_000,
                local_endpoint_id: local,
            })
            .await?
            .map_err(|refusal| format!("{refusal:?}"))?;
        Ok(Self {
            state,
            store,
            local,
            creator,
            space,
        })
    }

    /// Revokes `creator`'s `Process` capability in the shared Space.
    async fn withdraw_creator(&self) -> TestResult {
        self.store
            .grant_owned_space_member(OwnedMemberGrant {
                space_id: self.space,
                endpoint_id: self.creator,
                services: [false; 8],
                issued_at_ms: 4_000,
                local_endpoint_id: self.local,
            })
            .await?
            .map_err(|refusal| format!("{refusal:?}"))?;
        Ok(())
    }

    /// A manager as one daemon incarnation would run it; cancel the token to end it.
    fn manager(&self) -> (ProcessManager, CancellationToken) {
        let shutdown = CancellationToken::new();
        let manager = ProcessManager::new(
            (self.local, self.store.clone(), Arc::new(SystemClock)),
            (&self.state.0, Some(ProcessHosting::in_process())),
            shutdown.clone(),
        );
        (manager, shutdown)
    }
}

fn start(script: &str) -> TestResult<ProcessStart> {
    let spec = ProcessSpec::new(
        Some("/bin/sh".to_owned()),
        vec!["-c".to_owned(), script.to_owned()],
        Some("/".to_owned()),
    )?;
    Ok(ProcessStart::new(spec, false, MIB)?)
}

/// One end of an in-memory Process stream.
type End = (
    ma2a_services::FrameReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ma2a_services::FrameWriter<tokio::io::WriteHalf<tokio::io::DuplexStream>>,
);

/// Returns connected caller and host ends.
fn pipe() -> (End, End) {
    pipe_with(1 << 16)
}

/// Returns connected caller and host ends whose in-flight bytes are bounded by `capacity`.
fn pipe_with(capacity: usize) -> (End, End) {
    use ma2a_services::{FrameReader, FrameWriter};
    let (caller, host) = tokio::io::duplex(capacity);
    let (caller_read, caller_write) = tokio::io::split(caller);
    let (host_read, host_write) = tokio::io::split(host);
    (
        (
            FrameReader::new(caller_read),
            FrameWriter::new(caller_write),
        ),
        (FrameReader::new(host_read), FrameWriter::new(host_write)),
    )
}

fn request_id(byte: u8) -> TestResult<RequestId> {
    Ok(RequestId::try_from([byte; 16].as_slice())?)
}

/// Creates and brings up one Process as a remote `start` would.
async fn started(
    harness: &Harness,
    manager: &ProcessManager,
    (byte, script): (u8, &str),
) -> TestResult<ProcessRecord> {
    let start = start(script)?;
    let binding = (harness.creator, request_id(byte)?);
    let (creation, credentials) = manager.create(binding, &start).await?;
    let ProcessCreation::Created(record) = creation else {
        return Err("expected a new Process".into());
    };
    assert_eq!(record.state, ProcessState::Created);
    Ok(manager.bring_up(credentials, start).await?)
}

/// Waits until one Process reaches `state`.
async fn reaches(
    manager: &ProcessManager,
    id: ProcessId,
    state: ProcessState,
) -> TestResult<ProcessRecord> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let record = manager.record(id).await?.ok_or("missing Process")?;
        if record.state == state {
            return Ok(record);
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "stuck in {:?}",
            record.state
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
