use std::{
    fs::File,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use ma2a_core::{
    RequestId,
    service::{
        AgentId, AgentMessage, AgentTaskId, ArtifactRef, MAX_LOG_LIMIT, MessageRole, ProcessSpec,
        ProcessStart,
    },
};
use ma2a_runtime::{
    Runtime,
    current_user::CurrentUserRuntime,
    ipc::{IpcPaths, LocalApiServer},
    web::{SystemClock, WebAuthConfig, serve_connections},
};
use ma2a_store::{
    AgentDefinition, AgentTaskCreation, NewAgentTask, Repository, StoreConfig,
    protect_private_directory,
};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::super::{backend::Backend, http};

pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

pub(super) fn short_temp_dir(label: &str) -> TestResult<PathBuf> {
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ma2a-{label}-{:x}-{seq}", std::process::id()));
    std::fs::create_dir(&dir)?;
    protect_private_directory(&dir, "test state directory")?;
    Ok(dir)
}

pub(super) async fn spawn_http(
    backend: Backend,
) -> TestResult<(SocketAddr, CancellationToken, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let shared = http::Shared {
        backend,
        token: Arc::from("test-token"),
        origin: Arc::from(format!("http://{addr}")),
    };
    let stop = CancellationToken::new();
    let task = tokio::spawn(serve_connections(
        listener,
        http::router(shared),
        (stop.clone(), Duration::from_secs(2)),
    ));
    Ok((addr, stop, task))
}

pub(super) async fn http_get(
    addr: SocketAddr,
    path: &str,
) -> TestResult<(u16, Option<String>, Vec<u8>)> {
    let mut stream = TcpStream::connect(addr).await?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer test-token\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("no HTTP response head")?;
    let head = String::from_utf8_lossy(raw.get(..split).ok_or("bad head")?);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .ok_or("no status")?
        .parse()?;
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().to_owned());
    Ok((
        status,
        content_length,
        raw.get(split + 4..).unwrap_or_default().to_vec(),
    ))
}

struct RealFixture {
    state_dir: PathBuf,
    runtime: Option<Runtime>,
    stop: CancellationToken,
    tasks: [JoinHandle<()>; 2],
    addr: SocketAddr,
    task: AgentTaskId,
    blobs: PathBuf,
    _lock: File,
}

impl RealFixture {
    async fn start(label: &str) -> TestResult<Self> {
        let state_dir = short_temp_dir(label)?;
        let config = StoreConfig::new(&state_dir);
        let runtime = Runtime::start(config.clone()).await?;
        let control = CurrentUserRuntime::open_at(
            &state_dir,
            Arc::new(SystemClock::default()),
            WebAuthConfig::default(),
        )
        .await?;
        let paths = IpcPaths::new(&state_dir)?;
        paths.prepare()?;
        let lock = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(paths.lock_path())?;
        lock.lock()?;
        let server =
            LocalApiServer::bind_for_launch(paths.clone(), runtime.handle(), control, None).await?;
        let stop = CancellationToken::new();
        let ipc_token = stop.child_token();
        let ipc_task = tokio::spawn(async move {
            let _result = server.serve(ipc_token).await;
        });
        let local = runtime.handle().status().await?.endpoint_id();
        let mut repository = Repository::open(&config)?;
        let spec = ProcessSpec::new(Some("/bin/true".to_owned()), Vec::new(), None)?;
        let agent = AgentId::new("helper")?;
        repository.define_agent(
            &AgentDefinition {
                id: agent.clone(),
                description: String::new(),
                start: ProcessStart::new(spec, true, MAX_LOG_LIMIT)?.to_bytes()?,
                tools: 0,
                max_tasks: 1,
            },
            1,
        )?;
        repository.set_agent_enabled(&agent, (true, 2))?;
        let task = AgentTaskId::random()?;
        let AgentTaskCreation::Created(_) = repository
            .create_agent_task(&NewAgentTask {
                id: task,
                agent,
                binding: (local, RequestId::random()?),
                fingerprint: [0x11; 32],
                from_agent: None,
                input: AgentMessage::text(MessageRole::User, "work")?,
                created_ms: 2,
            })?
            .into_value()
        else {
            return Err("task was not created".into());
        };
        let blobs = state_dir.join("agents-v1").join("blobs");
        std::fs::create_dir_all(&blobs)?;
        let (addr, http_stop, http_task) = spawn_http(Backend {
            state_dir: state_dir.clone(),
            paths,
        })
        .await?;
        let forward_stop = stop.child_token();
        tokio::spawn(async move {
            forward_stop.cancelled().await;
            http_stop.cancel();
        });
        Ok(Self {
            state_dir,
            runtime: Some(runtime),
            stop,
            tasks: [ipc_task, http_task],
            addr,
            task,
            blobs,
            _lock: lock,
        })
    }

    fn record(&self, name: &str, content: &[u8]) -> TestResult<PathBuf> {
        let digest = *blake3::hash(content).as_bytes();
        let artifact = ArtifactRef::new(
            (name, "text/plain"),
            (u64::try_from(content.len())?, digest),
        )?;
        Repository::open(&StoreConfig::new(&self.state_dir))?
            .add_agent_artifact(self.task, (&artifact, 3))?;
        Ok(self
            .blobs
            .join(blake3::Hash::from_bytes(digest).to_hex().as_str()))
    }

    async fn get(&self, name: &str) -> TestResult<(u16, Option<String>, Vec<u8>)> {
        http_get(
            self.addr,
            &format!("/agents/helper/tasks/{}/artifacts/{name}", self.task),
        )
        .await
    }

    async fn stop(mut self) -> TestResult {
        self.stop.cancel();
        for task in &mut self.tasks {
            let _joined = task.await;
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown().await?;
        }
        Ok(())
    }
}

impl Drop for RealFixture {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        let _removed = std::fs::remove_dir_all(&self.state_dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_zero_byte_and_missing_blob_artifacts_never_return_200_over_http1() -> TestResult {
    let fixture = RealFixture::start("a2a-loc").await?;
    let empty_blob = fixture.record("empty.txt", b"")?;
    let _missing_nonempty = fixture.record("nonempty-missing.txt", b"expected bytes")?;
    let large_bytes = vec![0x5a_u8; 40_000];
    let large_blob = fixture.record("large.bin", &large_bytes)?;
    std::fs::write(&large_blob, &large_bytes)?;

    std::fs::write(&empty_blob, b"")?;
    let (status, length, body) = fixture.get("empty.txt").await?;
    assert_eq!((status, length.as_deref()), (200, Some("0")));
    assert!(body.is_empty());

    std::fs::remove_file(&empty_blob)?;
    assert_eq!(fixture.get("empty.txt").await?.0, 500);

    let target = fixture.state_dir.join("empty-target");
    std::fs::write(&target, b"")?;
    std::os::unix::fs::symlink(&target, &empty_blob)?;
    assert_eq!(fixture.get("empty.txt").await?.0, 500);

    std::fs::remove_file(&empty_blob)?;
    std::fs::write(&empty_blob, b"tampered")?;
    assert_eq!(fixture.get("empty.txt").await?.0, 500);
    assert_eq!(fixture.get("nonempty-missing.txt").await?.0, 500);

    let (status, length, body) = fixture.get("large.bin").await?;
    assert_eq!((status, length.as_deref()), (200, Some("40000")));
    assert_eq!(body, large_bytes);

    fixture.stop().await
}
