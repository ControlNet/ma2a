use std::{fs::File, path::PathBuf, sync::Arc};

use base64::Engine as _;
use ma2a_core::service::AgentTaskId;
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

use super::super::backend::Backend;
use super::local_tests::{TestResult, http_get, short_temp_dir, spawn_http};

struct StandIn {
    state_dir: PathBuf,
    paths: IpcPaths,
    serving: JoinHandle<()>,
    _lock: File,
}

impl StandIn {
    fn start(
        label: &str,
        handler: impl Fn(&str, u64) -> Value + Send + Sync + 'static,
    ) -> TestResult<Self> {
        let state_dir = short_temp_dir(label)?;
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
        let serving = tokio::spawn(serve_standin(listener, Arc::new(handler)));
        Ok(Self {
            state_dir,
            paths,
            serving,
            _lock: lock,
        })
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.serving.abort();
        let _removed = std::fs::remove_dir_all(&self.state_dir);
    }
}

async fn read_ipc(stream: &mut UnixStream) -> TestResult<(u64, Value)> {
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header).await?;
    let len = usize::try_from(u32::from_be_bytes(header[..4].try_into()?))?;
    if len > MAX_LOCAL_REQUEST_BYTES {
        return Err("request too large".into());
    }
    let mut payload = vec![0_u8; len];
    stream.read_exact(&mut payload).await?;
    Ok((
        u64::from_be_bytes(header[4..].try_into()?),
        serde_json::from_slice(&payload)?,
    ))
}

async fn write_ipc(stream: &mut UnixStream, correlation: u64, answer: &Value) -> TestResult {
    let payload = serde_json::to_vec(answer)?;
    let mut frame = u32::try_from(payload.len())?.to_be_bytes().to_vec();
    frame.extend_from_slice(&correlation.to_be_bytes());
    frame.extend_from_slice(&payload);
    stream.write_all(&frame).await?;
    Ok(())
}

type Handler = Arc<dyn Fn(&str, u64) -> Value + Send + Sync>;

async fn serve_standin(listener: UnixListener, handler: Handler) {
    let task_id = AgentTaskId::from_bytes([2; 16]).to_string();
    let empty_hash = blake3::hash(b"").to_hex().to_string();
    let small_hash = blake3::hash(b"hello").to_hex().to_string();
    let two_chunk_hash = blake3::hash(b"abcdef").to_hex().to_string();
    while let Ok((mut stream, _)) = listener.accept().await {
        let Ok((correlation, req)) = read_ipc(&mut stream).await else {
            continue;
        };
        let answer = match req.get("operation").and_then(Value::as_str) {
            Some("handshake") => json!({
                "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                "result": {"type": "handshake", "payload": {}},
            }),
            Some("agent_task_inspect") => json!({
                "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                "result": {"type": "agent_task", "payload": {
                    "task": {
                        "task_id": task_id, "agent": "helper",
                        "creator_endpoint_id": "01".repeat(32), "request_id": "09".repeat(16),
                        "state": "completed", "last_seq": 1,
                        "created_at_ms": 1, "started_at_ms": 1, "ended_at_ms": 2, "changed_at_ms": 2,
                    },
                    "input": {"role": "user", "parts": [{"kind": "text", "content": "hi"}]},
                    "artifacts": [
                        {"name": "unavailable.txt", "media_type": "text/plain", "size": 5, "blake3": small_hash},
                        {"name": "vanished.txt", "media_type": "text/plain", "size": 5, "blake3": small_hash},
                        {"name": "malformed.txt", "media_type": "text/plain", "size": 5, "blake3": small_hash},
                        {"name": "empty-noneof.txt", "media_type": "text/plain", "size": 5, "blake3": small_hash},
                        {"name": "forged.txt", "media_type": "text/plain", "size": 0, "blake3": empty_hash},
                        {"name": "small-ok.txt", "media_type": "text/plain", "size": 5, "blake3": small_hash},
                        {"name": "broken-tail.bin", "media_type": "application/octet-stream", "size": 6, "blake3": two_chunk_hash},
                    ],
                }},
            }),
            Some("agent_task_artifact_read") => {
                let name = req.get("name").and_then(Value::as_str).unwrap_or("");
                let offset = req.get("offset").and_then(Value::as_u64).unwrap_or(0);
                handler(name, offset)
            }
            _ => {
                json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "invalid_request"})
            }
        };
        let _written = write_ipc(&mut stream, correlation, &answer).await;
    }
}

fn chunk_ok((name, content): (&str, &str), (size, blake3): (u64, &str), eof: bool) -> Value {
    json!({
        "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
        "result": {"type": "agent_task_artifact", "payload": {
            "artifact": {"name": name, "media_type": "text/plain", "size": size, "blake3": blake3},
            "offset": 0, "content": content, "eof": eof,
        }},
    })
}

#[tokio::test]
async fn local_chunk_zero_errors_and_mismatches_map_to_http_status_and_tail_abort() -> TestResult {
    let small_hash = blake3::hash(b"hello").to_hex().to_string();
    let two_chunk_hash = blake3::hash(b"abcdef").to_hex().to_string();
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let standin = StandIn::start("a2a-std", move |name, offset| match (name, offset) {
        ("unavailable.txt", _) => {
            json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "unavailable"})
        }
        ("vanished.txt", _) => {
            json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "not_found"})
        }
        ("malformed.txt", _) => {
            json!({"version": LOCAL_API_VERSION, "request_id": null, "revision": 1, "result": {"type": "agent_task_artifact", "payload": {}}})
        }
        ("empty-noneof.txt", _) => chunk_ok((name, &b64(b"")), (5, &small_hash), false),
        ("forged.txt", _) => chunk_ok(
            (name, &b64(b"")),
            (0, &blake3::hash(b"wrong").to_hex()),
            true,
        ),
        ("small-ok.txt", _) => chunk_ok((name, &b64(b"hello")), (5, &small_hash), true),
        ("broken-tail.bin", 0) => chunk_ok((name, &b64(b"abc")), (6, &two_chunk_hash), false),
        ("broken-tail.bin", _) => chunk_ok((name, &b64(b"XYZ")), (6, &two_chunk_hash), true),
        _ => json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "not_found"}),
    })?;
    let (addr, stop, task) = spawn_http(Backend {
        state_dir: standin.state_dir.clone(),
        paths: standin.paths.clone(),
    })
    .await?;
    let task_id = AgentTaskId::from_bytes([2; 16]).to_string();
    let url = |name: &str| format!("/agents/helper/tasks/{task_id}/artifacts/{name}");

    assert_eq!(http_get(addr, &url("unavailable.txt")).await?.0, 503);
    assert_eq!(http_get(addr, &url("vanished.txt")).await?.0, 404);
    assert_eq!(http_get(addr, &url("malformed.txt")).await?.0, 500);
    assert_eq!(http_get(addr, &url("empty-noneof.txt")).await?.0, 500);
    assert_eq!(http_get(addr, &url("forged.txt")).await?.0, 500);

    let (status, length, body) = http_get(addr, &url("small-ok.txt")).await?;
    assert_eq!((status, length.as_deref()), (200, Some("5")));
    assert_eq!(body, b"hello");

    let (status, _length, body) = http_get(addr, &url("broken-tail.bin")).await?;
    assert_eq!(status, 200);
    assert!(
        body.len() < 6,
        "body must abort before Content-Length: {body:?}"
    );

    stop.cancel();
    let _server = task.await;
    Ok(())
}
