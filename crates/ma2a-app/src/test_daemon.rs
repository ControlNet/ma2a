//! A stand-in daemon for the remote calls: it completes the local handshake, opens every
//! stream, writes the target's scripted frames on it, and then falls silent. An empty script
//! stands for a target that accepted the stream and never said a word. Like a real daemon,
//! it refuses a stream aimed at its own Endpoint as invalid input.

use std::{fmt::Write as _, fs::File, path::PathBuf};

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        AgentAccept, AgentEvent, AgentEventBody, AgentId, AgentReply, AgentTaskId, AgentTaskState,
        AgentTaskSummary, FrameKind, ServiceError,
    },
};
use ma2a_runtime::{
    api::{LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES},
    ipc::IpcPaths,
};
use ma2a_services::FrameWriter;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The frames the target sends on every stream it is opened for.
pub(crate) type Script = Vec<(FrameKind, Vec<u8>)>;

/// A target that accepts with `accept` and answers `reply`.
pub(crate) fn answering(accept: AgentAccept, reply: &AgentReply) -> TestResult<Script> {
    Ok(vec![
        (FrameKind::Accept, accept.encode()?),
        (
            FrameKind::Outcome,
            AgentReply::encode_outcome(&Ok(reply.clone()))?,
        ),
    ])
}

/// Not a target's frames: the daemon refuses the stream with the local API error `error`, as
/// a Runtime that cannot open one does.
pub(crate) fn refusing(error: &str) -> Script {
    vec![(FrameKind::Open, error.as_bytes().to_vec())]
}

/// The stand-in Runtime's own Endpoint, which created every task the target answers with: the
/// identity point, a valid Ed25519 key.
const OWN: [u8; 32] = {
    let mut own = [0_u8; 32];
    own[0] = 1;
    own
};

/// Another Endpoint than the stand-in's own: the Ed25519 base point.
pub(crate) fn another_endpoint() -> TestResult<EndpointId> {
    let mut key = [0x66_u8; 32];
    key[0] = 0x58;
    Ok(EndpointId::try_from(key.as_slice())?)
}

/// A queued task of `agent` on the target.
pub(crate) fn summary(task: AgentTaskId, agent: &str) -> TestResult<AgentTaskSummary> {
    Ok(AgentTaskSummary::new(
        (task, AgentId::new(agent)?),
        (
            EndpointId::try_from(OWN.as_slice())?,
            RequestId::try_from([9; 16].as_slice())?,
        ),
        1_000,
    ))
}

/// A watch of `task` after event `after`, accepted.
fn watching(after: u32) -> TestResult<(FrameKind, Vec<u8>)> {
    let accept = AgentAccept::Watching(AgentTaskState::Running, after + 1);
    Ok((FrameKind::Accept, accept.encode()?))
}

/// Event `seq` of a task, reaching `state`.
fn event(seq: u32, state: AgentTaskState) -> TestResult<(FrameKind, Vec<u8>)> {
    let mut data = vec![1];
    let body = AgentEventBody::State(state, None);
    data.extend(AgentEvent::new(seq, 1_000, body).to_bytes()?);
    Ok((FrameKind::Data, data))
}

/// A watch after event 1 that the target loses after event 2, as when it restarts.
pub(crate) fn watch_lost() -> TestResult<Script> {
    let lost = AgentReply::encode_outcome(&Err(ServiceError::Unavailable))?;
    Ok(vec![
        watching(1)?,
        event(2, AgentTaskState::Running)?,
        (FrameKind::Outcome, lost),
    ])
}

/// A watch of `task` of the Agent `worker` after event 2 that sees it complete at event 3.
pub(crate) fn watch_completing(task: AgentTaskId) -> TestResult<Script> {
    let mut ended = summary(task, "worker")?;
    (ended.state, ended.last_seq) = (AgentTaskState::Completed, 3);
    let outcome = AgentReply::encode_outcome(&Ok(AgentReply::Summary(ended)))?;
    Ok(vec![
        watching(2)?,
        event(3, AgentTaskState::Completed)?,
        (FrameKind::Outcome, outcome),
    ])
}

pub(crate) struct Daemon {
    pub(crate) paths: IpcPaths,
    /// A well-formed Endpoint ID for the target.
    pub(crate) endpoint: String,
    /// The stand-in Runtime's own Endpoint, which no stream may target.
    pub(crate) own: String,
    pub(crate) state_dir: PathBuf,
    serving: JoinHandle<()>,
    /// Held like a running daemon holds it, so callers find one running.
    _lock: File,
}

impl Daemon {
    /// Serves a fresh state directory; must be called inside the test's runtime.
    pub(crate) fn start(label: &str, script: Script) -> TestResult<Self> {
        Self::paging(label, vec![script])
    }

    /// Like [`Self::start`], with the `n`th stream answered by the `n`th script, and every
    /// stream after the last by the last.
    pub(crate) fn paging(label: &str, scripts: Vec<Script>) -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let state_dir = std::env::temp_dir().join(format!(
            "ma2a-standin-{label}-{:016x}",
            u64::from_le_bytes(nonce)
        ));
        std::fs::create_dir(&state_dir)?;
        let paths = IpcPaths::new(&state_dir)?;
        paths.prepare()?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.lock_path())?;
        lock.lock()?;
        let listener = UnixListener::bind(paths.socket_path())?;
        let hex = |key: &[u8]| {
            key.iter().fold(String::new(), |mut hex, byte| {
                let _written = write!(hex, "{byte:02x}");
                hex
            })
        };
        let endpoint = hex(iroh::SecretKey::generate().public().as_bytes());
        let own = hex(&OWN);
        let serving = tokio::spawn(serve(listener, (scripts, own.clone())));
        Ok(Self {
            paths,
            endpoint,
            own,
            state_dir,
            serving,
            _lock: lock,
        })
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.serving.abort();
        let _removed = std::fs::remove_dir_all(&self.state_dir);
    }
}

async fn serve(listener: UnixListener, (scripts, own): (Vec<Script>, String)) {
    let mut opened = 0;
    while let Ok((mut stream, _)) = listener.accept().await {
        let Ok((correlation, request)) = read(&mut stream).await else {
            continue;
        };
        let mut script = None;
        let version = LOCAL_API_VERSION;
        let answer = match request.get("operation").and_then(Value::as_str) {
            // Not a lifecycle report, so a caller asks the handshake instead.
            _ if request.get("ma2a_lifecycle").is_some() => {
                json!({"version": version, "request_id": null, "error": "invalid_request"})
            }
            Some("handshake") => json!({
                "version": version, "request_id": null, "revision": 1,
                "result": {"type": "handshake", "payload": {}},
            }),
            Some("endpoint_info") => json!({
                "version": version, "request_id": null, "revision": 1,
                "result": {"type": "endpoint_info", "payload": {"endpoint_id": own}},
            }),
            Some("stream_open") if request.get("target_endpoint_id") == Some(&json!(own)) => {
                json!({"version": version, "request_id": null, "error": "invalid_input"})
            }
            Some("stream_open") => {
                script = scripts.get(opened).or_else(|| scripts.last()).cloned();
                opened += 1;
                match script.as_deref() {
                    Some([(FrameKind::Open, error)]) => json!({
                        "version": version, "request_id": null,
                        "error": String::from_utf8_lossy(error),
                    }),
                    _ => json!({
                        "version": version, "request_id": null, "revision": 1,
                        "result": {"type": "stream_opened", "payload": {}},
                    }),
                }
            }
            _ => json!({
                "version": version, "request_id": null, "revision": 1,
                "result": {"type": "stream_opened", "payload": {}},
            }),
        };
        if write(&mut stream, correlation, &answer).await.is_err() {
            continue;
        }
        if answer.pointer("/result/type") == Some(&json!("stream_opened")) {
            tokio::spawn(target(stream, script.take().unwrap_or_default()));
        }
    }
}

/// Sends `script` as the target, then holds the stream open without another word.
async fn target(stream: UnixStream, script: Script) {
    let (_read, write) = stream.into_split();
    let mut writer = FrameWriter::new(write);
    for (kind, payload) in &script {
        if writer.frame(*kind, payload).await.is_err() {
            return;
        }
    }
    if writer.flush().await.is_ok() {
        std::future::pending::<()>().await;
    }
}

/// Reads one local API request frame: a 4-byte length, an 8-byte correlation, the JSON.
async fn read(stream: &mut UnixStream) -> TestResult<(u64, Value)> {
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).await?;
    let (length, correlation) = header.split_at(4);
    let length = usize::try_from(u32::from_be_bytes(length.try_into()?))?;
    if length > MAX_LOCAL_REQUEST_BYTES {
        return Err("request too large".into());
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    Ok((
        u64::from_be_bytes(correlation.try_into()?),
        serde_json::from_slice(&payload)?,
    ))
}

async fn write(stream: &mut UnixStream, correlation: u64, answer: &Value) -> TestResult {
    let payload = serde_json::to_vec(answer)?;
    let mut frame = u32::try_from(payload.len())?.to_be_bytes().to_vec();
    frame.extend_from_slice(&correlation.to_be_bytes());
    frame.extend_from_slice(&payload);
    stream.write_all(&frame).await?;
    Ok(())
}
