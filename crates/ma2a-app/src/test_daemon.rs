//! A stand-in daemon for the remote calls: it completes the local handshake, opens every
//! stream, writes the target's scripted frames on it, and then falls silent. An empty script
//! stands for a target that accepted the stream and never said a word. Like a real daemon,
//! it refuses a stream aimed at its own Endpoint as invalid input.

use std::{fmt::Write as _, fs::File, path::PathBuf};

use ma2a_core::{
    EndpointId, RequestId,
    service::{AgentAccept, AgentId, AgentReply, AgentTaskId, AgentTaskSummary, FrameKind},
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

/// A queued task of `agent` on the target.
pub(crate) fn summary(task: AgentTaskId, agent: &str) -> TestResult<AgentTaskSummary> {
    // The identity point is a valid Ed25519 key.
    let mut creator = [0_u8; 32];
    creator[0] = 1;
    Ok(AgentTaskSummary::new(
        (task, AgentId::new(agent)?),
        (
            EndpointId::try_from(creator.as_slice())?,
            RequestId::try_from([9; 16].as_slice())?,
        ),
        1_000,
    ))
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
        let key = || {
            let key = iroh::SecretKey::generate().public();
            key.as_bytes().iter().fold(String::new(), |mut hex, byte| {
                let _written = write!(hex, "{byte:02x}");
                hex
            })
        };
        let (endpoint, own) = (key(), key());
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
            Some("stream_open") if request.get("target_endpoint_id") == Some(&json!(own)) => {
                json!({"version": version, "request_id": null, "error": "invalid_input"})
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
            let script = scripts.get(opened).or_else(|| scripts.last()).cloned();
            opened += 1;
            tokio::spawn(target(stream, script.unwrap_or_default()));
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
