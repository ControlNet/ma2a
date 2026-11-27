//! `ListTasks` skips local tasks that vanish between `details()` and `view::of`.

use std::fs::File;

use ma2a_core::service::{AgentId, AgentTaskId};
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

use super::list;
use crate::a2a::{
    backend::{Backend, Route},
    view::Served,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct LocalStandIn {
    state_dir: std::path::PathBuf,
    paths: IpcPaths,
    serving: JoinHandle<()>,
    _lock: File,
}

impl LocalStandIn {
    fn start(vanished_events: AgentTaskId) -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let state_dir = std::env::temp_dir().join(format!(
            "ma2a-list-local-{:016x}",
            u64::from_le_bytes(nonce)
        ));
        std::fs::create_dir(&state_dir)?;
        ma2a_store::protect_private_directory(&state_dir, "test state directory")?;
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
        let serving = tokio::spawn(serve_local(listener, vanished_events));
        Ok(Self {
            state_dir,
            paths,
            serving,
            _lock: lock,
        })
    }
}

impl Drop for LocalStandIn {
    fn drop(&mut self) {
        self.serving.abort();
        let _removed = std::fs::remove_dir_all(&self.state_dir);
    }
}

const fn task_id(byte: u8) -> AgentTaskId {
    AgentTaskId::from_bytes([byte; 16])
}

fn summary_json(byte: u8) -> Value {
    json!({
        "task_id": task_id(byte).to_string(),
        "agent": "worker",
        "creator_endpoint_id": "01".repeat(32),
        "request_id": "09".repeat(16),
        "state": "waiting",
        "last_seq": 1,
        "created_at_ms": 1_000,
        "started_at_ms": 1_000,
        "ended_at_ms": null,
        "changed_at_ms": 2_000,
    })
}

async fn read_frame(stream: &mut UnixStream) -> TestResult<(u64, Value)> {
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

async fn write_frame(stream: &mut UnixStream, correlation: u64, answer: &Value) -> TestResult {
    let payload = serde_json::to_vec(answer)?;
    let mut frame = u32::try_from(payload.len())?.to_be_bytes().to_vec();
    frame.extend_from_slice(&correlation.to_be_bytes());
    frame.extend_from_slice(&payload);
    stream.write_all(&frame).await?;
    Ok(())
}

async fn serve_local(listener: UnixListener, vanished_events: AgentTaskId) {
    let vanished = vanished_events.to_string();
    while let Ok((mut stream, _)) = listener.accept().await {
        let Ok((correlation, request)) = read_frame(&mut stream).await else {
            continue;
        };
        let answer = match request.get("operation").and_then(Value::as_str) {
            Some("handshake") => json!({
                "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                "result": {"type": "handshake", "payload": {}},
            }),
            Some("agent_task_list") => json!({
                "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                "result": {"type": "agent_tasks", "payload": {
                    "tasks": [summary_json(1), summary_json(2), summary_json(3)],
                    "next": null,
                }},
            }),
            Some("agent_task_inspect") => {
                let id = request.get("task_id").and_then(Value::as_str).unwrap_or("");
                let byte = if id == task_id(1).to_string() {
                    1
                } else if id == task_id(2).to_string() {
                    2
                } else {
                    3
                };
                json!({
                    "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                    "result": {"type": "agent_task", "payload": {
                        "task": summary_json(byte),
                        "input": {"role": "user", "parts": [{"kind": "text", "content": "hi"}]},
                        "artifacts": [],
                    }},
                })
            }
            Some("agent_task_events") => {
                let id = request.get("task_id").and_then(Value::as_str).unwrap_or("");
                if id == vanished {
                    json!({
                        "version": LOCAL_API_VERSION, "request_id": null,
                        "error": "unknown_task",
                    })
                } else {
                    json!({
                        "version": LOCAL_API_VERSION, "request_id": null, "revision": 1,
                        "result": {"type": "agent_task_events", "payload": {
                            "events": [{
                                "seq": 1,
                                "at_ms": 2_000,
                                "kind": "state",
                                "state": "waiting",
                                "message": {"role": "agent", "parts": [{"kind": "text", "content": "ok"}]},
                            }],
                            "more": false,
                        }},
                    })
                }
            }
            _ => {
                json!({"version": LOCAL_API_VERSION, "request_id": null, "error": "invalid_request"})
            }
        };
        let _written = write_frame(&mut stream, correlation, &answer).await;
    }
}

#[tokio::test]
async fn a_task_that_vanishes_before_reading_its_events_is_skipped() -> TestResult {
    let daemon = LocalStandIn::start(task_id(1))?;
    let backend = Backend {
        state_dir: daemon.state_dir.clone(),
        paths: daemon.paths.clone(),
    };
    let route = Route::Local(AgentId::new("worker")?);
    let served = Served {
        backend: &backend,
        route: &route,
        base: "http://127.0.0.1",
    };

    let page = list(served, &json!({"historyLength": 2, "pageSize": 2}))
        .await
        .map_err(|error| error.message)?;
    let ids: Vec<&str> = page
        .get("tasks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, [task_id(2).to_string(), task_id(3).to_string()]);
    assert_eq!(page.get("totalSize"), Some(&json!(2)));
    assert_eq!(page.get("nextPageToken"), Some(&json!("")));
    Ok(())
}
