//! Tool calls against a fake local daemon that never answers what it opened.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ma2a_agent::tools::{AgentTaskTarget, AgentWait, ProxyOpen};
use ma2a_core::{
    RequestId,
    service::{AgentId, AgentTaskId},
};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::Call;
use crate::{
    agents::AgentManager,
    api,
    clock::SystemClock,
    ipc::{self, FrameRef, IpcPaths},
    processes::ProcessManager,
    store::StoreClient,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// A private state directory removed on drop.
struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let nonce = u64::from_le_bytes(nonce);
        let path = std::env::temp_dir().join(format!("ma2a-tools-{label}-{nonce:016x}"));
        std::fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        ma2a_store::protect_private_directory(&path, "test state directory")?;
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

/// An Agent manager whose tools reach a daemon that opens every stream and then falls silent.
struct Fixture {
    state: TempState,
    manager: AgentManager,
    /// Streams the daemon has opened and then left silent.
    opened: Arc<AtomicUsize>,
    daemon: tokio::task::JoinHandle<()>,
}

impl Fixture {
    fn new(label: &str) -> TestResult<Self> {
        let state = TempState::new(label)?;
        let (store, _requests) = mpsc::channel(1);
        let store = StoreClient::new(store);
        let local = iroh::SecretKey::generate().public().into();
        let shutdown = CancellationToken::new();
        let processes = ProcessManager::new(
            (local, store.clone(), Arc::new(SystemClock)),
            (&state.0, None),
            shutdown.clone(),
        );
        let manager = AgentManager::new(
            (local, store, Arc::new(SystemClock)),
            (&state.0, processes),
            shutdown,
        );
        let paths = IpcPaths::new(&state.0)?;
        paths.prepare()?;
        let listener = ipc::bind(&paths)?;
        manager.install_loopback(paths);
        let opened = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&opened);
        let daemon = tokio::spawn(async move {
            let _ended = silent_daemon(&listener, &counted).await;
        });
        Ok(Self {
            state,
            manager,
            opened,
            daemon,
        })
    }

    fn call(&self) -> TestResult<Call<'_>> {
        Ok(Call {
            manager: &self.manager,
            task: AgentTaskId::random()?,
            id: "call-1",
            agent: AgentId::new("caller")?,
            workspace: self.state.0.clone(),
            request_id: RequestId::random()?,
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.daemon.abort();
    }
}

/// Answers the handshake, upgrades every `stream_open`, and never sends a frame on it.
async fn silent_daemon(listener: &ipc::PlatformListener, opened: &AtomicUsize) -> TestResult {
    loop {
        let mut stream = ipc::accept(listener).await?;
        let frame = ipc::read_frame(&mut stream, api::MAX_LOCAL_REQUEST_BYTES).await?;
        let request: Value = serde_json::from_slice(&frame.payload)?;
        let answer = match request.get("operation").and_then(Value::as_str) {
            Some("handshake") => "handshake",
            _ => "stream_opened",
        };
        let payload = format!(
            r#"{{"version":{},"request_id":null,"revision":1,"result":{{"type":"{answer}","payload":{{}}}}}}"#,
            api::LOCAL_API_VERSION
        );
        let reply = FrameRef {
            correlation: frame.correlation,
            payload: payload.as_bytes(),
            maximum: api::MAX_LOCAL_RESPONSE_BYTES,
        };
        ipc::write_frame(&mut stream, reply).await?;
        if answer == "stream_opened" {
            opened.fetch_add(1, Ordering::Relaxed);
        }
        tokio::spawn(async move {
            let _held = stream;
            std::future::pending::<()>().await;
        });
    }
}

fn target() -> String {
    crate::agents::convert::hex(iroh::SecretKey::generate().public().as_bytes())
}

#[tokio::test(start_paused = true)]
async fn agent_get_from_a_silent_target_times_out() -> TestResult {
    // Given
    let fixture = Fixture::new("agent-get")?;
    let call = fixture.call()?;
    let args = AgentTaskTarget {
        endpoint: target(),
        agent: "helper".to_owned(),
        task: AgentTaskId::random()?.to_string(),
    };

    // When
    let answered = tokio::time::timeout(Duration::from_secs(60), call.agent_get(args)).await;

    // Then
    let Ok(Err(error)) = answered else {
        return Err("agent.get waited on a target that never answers".into());
    };
    assert_eq!(error.code, "timed_out");
    assert_eq!(fixture.opened.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn agent_wait_reads_no_more_views_once_its_deadline_passed() -> TestResult {
    // Given
    let fixture = Fixture::new("agent-wait")?;
    let call = fixture.call()?;
    let args = AgentWait {
        endpoint: target(),
        agent: "helper".to_owned(),
        task: AgentTaskId::random()?.to_string(),
        timeout_ms: Some(1_000),
    };
    let began = tokio::time::Instant::now();

    // When
    let answered = tokio::time::timeout(Duration::from_secs(120), call.agent_wait(args)).await;

    // Then: the one view read begun before the deadline is the last.
    let Ok(Err(error)) = answered else {
        return Err("agent.wait did not end with the view read's failure".into());
    };
    assert_eq!(error.code, "timed_out");
    assert_eq!(began.elapsed(), Duration::from_secs(30));
    assert_eq!(fixture.opened.load(Ordering::Relaxed), 1);
    Ok(())
}

#[tokio::test]
async fn proxy_open_to_port_zero_fails_before_listening() -> TestResult {
    // Given
    let fixture = Fixture::new("proxy-port")?;
    let call = fixture.call()?;
    let args = ProxyOpen {
        endpoint: target(),
        host: "localhost".to_owned(),
        port: 0,
    };

    // When
    let opened = call.proxy_open(args).await;

    // Then
    let error = opened
        .err()
        .ok_or("a listener was answered for port zero")?;
    assert_eq!(error.code, "invalid_tool");
    assert!(fixture.manager.coordination().listeners.is_empty());
    Ok(())
}
