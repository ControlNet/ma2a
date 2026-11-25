//! A stand-in daemon for the remote commands: it completes the local handshake, opens every
//! stream it is asked for, and then never sends a frame on it, as a target that accepted the
//! stream and fell silent.

use std::{fs::File, path::PathBuf};

use ma2a_runtime::{
    api::{LOCAL_API_VERSION, MAX_LOCAL_REQUEST_BYTES},
    ipc::IpcPaths,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{UnixListener, UnixStream},
    task::JoinHandle,
};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) struct SilentTarget {
    pub(super) paths: IpcPaths,
    /// A well-formed Endpoint ID for the target.
    pub(super) endpoint: String,
    pub(super) state_dir: PathBuf,
    serving: JoinHandle<()>,
    /// Held like a running daemon holds it, so callers find one running.
    _lock: File,
}

impl SilentTarget {
    /// Serves a fresh state directory; must be called inside the test's runtime.
    pub(super) fn start(label: &str) -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let state_dir = std::env::temp_dir().join(format!(
            "ma2a-silent-{label}-{:016x}",
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
        let serving = tokio::spawn(serve(listener));
        let endpoint = iroh::SecretKey::generate().public();
        let endpoint = endpoint.as_bytes().iter().map(|byte| format!("{byte:02x}"));
        Ok(Self {
            paths,
            endpoint: endpoint.collect(),
            state_dir,
            serving,
            _lock: lock,
        })
    }
}

impl Drop for SilentTarget {
    fn drop(&mut self) {
        self.serving.abort();
        let _removed = std::fs::remove_dir_all(&self.state_dir);
    }
}

async fn serve(listener: UnixListener) {
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
            _ => json!({
                "version": version, "request_id": null, "revision": 1,
                "result": {"type": "stream_opened", "payload": {}},
            }),
        };
        if write(&mut stream, correlation, &answer).await.is_ok()
            && answer.pointer("/result/type") == Some(&json!("stream_opened"))
        {
            // Held open without another word.
            tokio::spawn(async move {
                let _held = stream;
                std::future::pending::<()>().await;
            });
        }
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
