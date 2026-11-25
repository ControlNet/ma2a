//! Tool calls against a fake local daemon that never answers what it opened.

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ma2a_agent::tools::{
    AgentArtifactArgs, AgentTaskTarget, AgentWait, FilesTarget, ProcessTarget, ProxyOpen,
    TransferGet,
};
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
    /// Streams the daemon has taken and then left silent.
    opened: Arc<AtomicUsize>,
    daemon: tokio::task::JoinHandle<()>,
}

/// The frames a target sends on every stream once it is upgraded.
type Script = Vec<(ma2a_core::service::FrameKind, Vec<u8>)>;

impl Fixture {
    fn new(label: &str, upgrade: Option<Duration>) -> TestResult<Self> {
        Self::answering(label, (upgrade, Vec::new()))
    }

    /// A daemon that upgrades every stream after `upgrade` and sends `script` on it as the
    /// target, then falls silent.
    fn answering(label: &str, (upgrade, script): (Option<Duration>, Script)) -> TestResult<Self> {
        let state = TempState::new(label)?;
        let (store, _requests) = mpsc::channel(1);
        let store = StoreClient::new(store);
        let local = local();
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
            let _ended = silent_daemon(&listener, (&counted, upgrade), script).await;
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
            request_id: RequestId::try_from(OPENED.as_slice())?,
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.daemon.abort();
    }
}

/// Answers the handshake and takes every `stream_open`, then sends only `script` on it: with
/// `upgrade`, after upgrading it once that delay passed; without, not even the upgrade reply.
async fn silent_daemon(
    listener: &ipc::PlatformListener,
    (opened, upgrade): (&AtomicUsize, Option<Duration>),
    script: Script,
) -> TestResult {
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
        let delay = if answer == "handshake" {
            Some(Duration::ZERO)
        } else {
            opened.fetch_add(1, Ordering::Relaxed);
            upgrade
        };
        let script = if answer == "handshake" {
            Vec::new()
        } else {
            script.clone()
        };
        tokio::spawn(async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
                let reply = FrameRef {
                    correlation: frame.correlation,
                    payload: payload.as_bytes(),
                    maximum: api::MAX_LOCAL_RESPONSE_BYTES,
                };
                let _written = ipc::write_frame(&mut stream, reply).await;
                let mut target = ma2a_services::FrameWriter::new(&mut stream);
                for (kind, payload) in &script {
                    let _written = target.frame(*kind, payload).await;
                }
                let _flushed = target.flush().await;
            }
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
    let fixture = Fixture::new("agent-get", Some(Duration::ZERO))?;
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
    let fixture = Fixture::new("agent-wait", Some(Duration::ZERO))?;
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

#[tokio::test(start_paused = true)]
async fn read_only_calls_to_a_silent_target_time_out() -> TestResult {
    // Given
    let fixture = Fixture::new("silent-reads", Some(Duration::ZERO))?;
    let call = fixture.call()?;
    let files = |path: &str| FilesTarget {
        endpoint: target(),
        root: "home".to_owned(),
        path: path.to_owned(),
        after: None,
    };
    let process = ProcessTarget {
        endpoint: target(),
        id: ma2a_core::service::ProcessId::from_bytes([1; 16]).to_string(),
    };

    // When
    let bound = Duration::from_secs(60);
    let stat = tokio::time::timeout(bound, call.files_stat(files("a"))).await;
    let list = tokio::time::timeout(bound, call.files_list(files("b"))).await;
    let inspect = tokio::time::timeout(bound, call.process_inspect(process)).await;

    // Then: each is `timed_out` after 30 s instead of holding its call forever.
    for (tool, answered) in [
        ("files.stat", stat),
        ("files.list", list),
        ("process.inspect", inspect),
    ] {
        let Ok(Err(error)) = answered else {
            return Err(format!("{tool} waited on a target that never answers").into());
        };
        assert_eq!(error.code, "timed_out", "{tool}");
    }
    assert_eq!(fixture.opened.load(Ordering::Relaxed), 3);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn downloads_from_a_silent_target_time_out() -> TestResult {
    // Given
    let fixture = Fixture::new("silent-downloads", Some(Duration::ZERO))?;
    let call = fixture.call()?;
    let get = TransferGet {
        endpoint: target(),
        root: "home".to_owned(),
        path: "a".to_owned(),
        to: "a".to_owned(),
    };
    let artifact = AgentArtifactArgs {
        endpoint: target(),
        agent: "helper".to_owned(),
        task: AgentTaskId::random()?.to_string(),
        name: "report.txt".to_owned(),
        to: "b".to_owned(),
    };

    // When
    let bound = Duration::from_secs(60);
    let got = tokio::time::timeout(bound, call.transfer_get(get)).await;
    let fetched = tokio::time::timeout(bound, call.agent_artifact(artifact)).await;

    // Then: a target that never accepts is `timed_out` after 30 s.
    for (tool, answered) in [("transfer.get", got), ("agent.artifact", fetched)] {
        let Ok(Err(error)) = answered else {
            return Err(format!("{tool} waited on a target that never answers").into());
        };
        assert_eq!(error.code, "timed_out", "{tool}");
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_download_waits_one_deadline_from_opening_through_its_accept() -> TestResult {
    // A local daemon that never upgrades the stream, and one that upgrades it after 20 s.
    for (label, upgrade) in [
        ("download-no-upgrade", None),
        ("download-late-upgrade", Some(Duration::from_secs(20))),
    ] {
        // Given
        let fixture = Fixture::new(label, upgrade)?;
        let call = fixture.call()?;
        let get = TransferGet {
            endpoint: target(),
            root: "home".to_owned(),
            path: "a".to_owned(),
            to: "a".to_owned(),
        };
        let artifact = AgentArtifactArgs {
            endpoint: target(),
            agent: "helper".to_owned(),
            task: AgentTaskId::random()?.to_string(),
            name: "report.txt".to_owned(),
            to: "b".to_owned(),
        };

        // When
        let bound = Duration::from_secs(60);
        let began = tokio::time::Instant::now();
        let got = tokio::time::timeout(bound, call.transfer_get(get)).await;
        let got_after = began.elapsed();
        let fetched = tokio::time::timeout(bound, call.agent_artifact(artifact)).await;
        let fetched_after = began.elapsed() - got_after;

        // Then: each is `timed_out` 30 s after it began opening, whatever the upgrade took.
        for (tool, answered, after) in [
            ("transfer.get", got, got_after),
            ("agent.artifact", fetched, fetched_after),
        ] {
            let Ok(Err(error)) = answered else {
                return Err(format!("{label}: {tool} outlived its deadline").into());
            };
            assert_eq!(error.code, "timed_out", "{label}: {tool}");
            assert_eq!(after, Duration::from_secs(30), "{label}: {tool}");
        }
    }
    Ok(())
}

#[tokio::test]
async fn proxy_open_to_port_zero_fails_before_listening() -> TestResult {
    // Given
    let fixture = Fixture::new("proxy-port", Some(Duration::ZERO))?;
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

#[tokio::test]
async fn ending_the_task_closes_a_proxy_listener_while_a_connection_is_opening() -> TestResult {
    // Given: a daemon that takes each connection's stream open and never answers it.
    let fixture = Fixture::new("proxy-opening", None)?;
    let call = fixture.call()?;
    let args = ProxyOpen {
        endpoint: target(),
        host: "localhost".to_owned(),
        port: 80,
    };
    let listening = call.proxy_open(args).await.map_err(|error| error.message)?;
    let listen: std::net::SocketAddr = listening
        .get("listen")
        .and_then(Value::as_str)
        .ok_or("no listen address")?
        .parse()?;
    let _client = tokio::net::TcpStream::connect(listen).await?;
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.opened.load(Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;

    // When
    fixture.manager.task_ended(call.task);

    // Then: the listener and its work end without the open ever being answered.
    fixture.manager.0.tasks.close();
    tokio::time::timeout(Duration::from_secs(5), fixture.manager.0.tasks.wait())
        .await
        .map_err(|_| "the listener outlived its task")?;
    assert!(tokio::net::TcpStream::connect(listen).await.is_err());
    Ok(())
}

#[tokio::test]
async fn proxy_open_to_an_invalid_or_own_endpoint_fails_before_listening() -> TestResult {
    // Given
    let fixture = Fixture::new("proxy-endpoint", Some(Duration::ZERO))?;
    let call = fixture.call()?;
    let own = crate::agents::convert::hex(fixture.manager.local().as_bytes());
    let open = |endpoint: String| ProxyOpen {
        endpoint,
        host: "localhost".to_owned(),
        port: 80,
    };

    // When
    let invalid = call.proxy_open(open("not-an-endpoint".to_owned())).await;
    let itself = call.proxy_open(open(own)).await;

    // Then: neither is answered with a listener that drops every connection.
    let invalid = invalid
        .err()
        .ok_or("a listener was answered for an invalid endpoint")?;
    let itself = itself
        .err()
        .ok_or("a listener was answered for this Runtime")?;
    assert_eq!(invalid.code, "invalid_tool");
    assert_eq!(itself.code, "invalid_input");
    assert!(fixture.manager.coordination().listeners.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_proxy_connection_still_opening_does_not_hold_the_next_one() -> TestResult {
    // Given: a daemon that takes each connection's stream open and never answers it.
    let fixture = Fixture::new("proxy-parallel", None)?;
    let call = fixture.call()?;
    let args = ProxyOpen {
        endpoint: target(),
        host: "localhost".to_owned(),
        port: 80,
    };
    let listening = call.proxy_open(args).await.map_err(|error| error.message)?;
    let listen: std::net::SocketAddr = listening
        .get("listen")
        .and_then(Value::as_str)
        .ok_or("no listen address")?
        .parse()?;

    // When: two connections arrive while the first is still opening.
    let _first = tokio::net::TcpStream::connect(listen).await?;
    let _second = tokio::net::TcpStream::connect(listen).await?;

    // Then: both are being opened at once.
    let opened = tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.opened.load(Ordering::Relaxed) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    fixture.manager.task_ended(call.task);
    opened.map_err(|_| "the second connection waited for the first to open")?;
    Ok(())
}

/// A target that accepts with `accept` and answers `reply`.
fn agent_answer(
    accept: ma2a_core::service::AgentAccept,
    reply: &ma2a_core::service::AgentReply,
) -> TestResult<Script> {
    use ma2a_core::service::{AgentReply, FrameKind};
    Ok(vec![
        (FrameKind::Accept, accept.encode()?),
        (
            FrameKind::Outcome,
            AgentReply::encode_outcome(&Ok(reply.clone()))?,
        ),
    ])
}

/// This Runtime's own Endpoint, which creates every task its tools submit.
fn local() -> ma2a_core::EndpointId {
    iroh::SecretKey::from_bytes(&[7; 32]).public().into()
}

/// The `RequestId` every tool call here opens its mutations under.
const OPENED: [u8; 16] = [9; 16];

/// A completed task of `agent` on the target, created by a call opened under [`OPENED`].
fn ended(task: AgentTaskId, agent: &str) -> TestResult<ma2a_core::service::AgentTaskSummary> {
    let mut summary = ma2a_core::service::AgentTaskSummary::new(
        (task, AgentId::new(agent)?),
        (local(), RequestId::try_from(OPENED.as_slice())?),
        1_000,
    );
    summary.state = ma2a_core::service::AgentTaskState::Completed;
    Ok(summary)
}

fn task_details(task: AgentTaskId, agent: &str) -> TestResult<ma2a_core::service::AgentReply> {
    use ma2a_core::service::{
        AgentMessage, AgentReply, AgentTaskDetails, MessagePart, MessageRole,
    };
    let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".to_owned())])?;
    let details = AgentTaskDetails::new(ended(task, agent)?, input);
    Ok(AgentReply::Details(Box::new(details)))
}

/// Runs one tool against a target answering `script`, and returns its error code, if any.
async fn answered_code<F, R>(label: &str, script: Script, tool: F) -> TestResult<Option<String>>
where
    F: AsyncFnOnce(&Call<'_>) -> Result<R, ma2a_agent::protocol::ToolError>,
{
    let fixture = Fixture::answering(label, (Some(Duration::ZERO), script))?;
    let call = fixture.call()?;
    let answered = tokio::time::timeout(Duration::from_secs(60), tool(&call)).await?;
    Ok(answered.err().map(|error| error.code))
}

#[tokio::test]
async fn agent_tools_take_only_the_task_they_named() -> TestResult {
    use ma2a_core::service::{AgentAccept, AgentReply};
    let (a, b) = (
        AgentTaskId::from_bytes([1; 16]),
        AgentTaskId::from_bytes([2; 16]),
    );
    let endpoint = target();
    let named = AgentTaskTarget {
        endpoint: endpoint.clone(),
        agent: "helper".to_owned(),
        task: a.to_string(),
    };
    let wait = AgentWait {
        endpoint: endpoint.clone(),
        agent: "helper".to_owned(),
        task: a.to_string(),
        timeout_ms: Some(1_000),
    };
    let admitted = |reply: TestResult<AgentReply>| agent_answer(AgentAccept::Admitted, &reply?);

    // The task named is taken.
    let own = admitted(task_details(a, "helper"))?;
    let get = answered_code("own-get", own.clone(), async |call| {
        call.agent_get(named.clone()).await
    });
    assert_eq!(get.await?, None);
    let waited = answered_code("own-wait", own, async |call| {
        call.agent_wait(wait.clone()).await
    });
    assert_eq!(waited.await?, None);

    // Another task, or the same task of another Agent, is not.
    for (case, reply) in [
        ("other-task", task_details(b, "helper")),
        ("other-agent", task_details(a, "other")),
    ] {
        let script = admitted(reply)?;
        let get = answered_code(case, script.clone(), async |call| {
            call.agent_get(named.clone()).await
        });
        assert_eq!(get.await?.as_deref(), Some("invalid_tool"), "get: {case}");
        let waited = answered_code(case, script, async |call| {
            call.agent_wait(wait.clone()).await
        });
        assert_eq!(
            waited.await?.as_deref(),
            Some("invalid_tool"),
            "wait: {case}"
        );
    }

    // A submit takes only the task its accept bound it to, of the Agent it named.
    let submit = ma2a_agent::tools::AgentSubmitArgs {
        endpoint,
        agent: "helper".to_owned(),
        message: ma2a_agent::protocol::Message::user_text("hi"),
    };
    for (case, bound, agent, code) in [
        ("submit-own", a, "helper", None),
        ("submit-other-task", b, "helper", Some("invalid_tool")),
        ("submit-other-agent", a, "other", Some("invalid_tool")),
    ] {
        let reply = AgentReply::Summary(ended(a, agent)?);
        let script = agent_answer(AgentAccept::Submitted(bound, false), &reply)?;
        let submitted = answered_code(case, script, async |call| {
            call.agent_submit(submit.clone()).await
        });
        assert_eq!(submitted.await?.as_deref(), code, "{case}");
    }
    // Nor the task another submission created, bound and answered in full.
    let mut another = ended(a, "helper")?;
    another.request_id = RequestId::try_from([8; 16].as_slice())?;
    let script = agent_answer(
        AgentAccept::Submitted(a, false),
        &AgentReply::Summary(another),
    )?;
    let submitted = answered_code("submit-another-request", script, async |call| {
        call.agent_submit(submit.clone()).await
    });
    assert_eq!(submitted.await?.as_deref(), Some("invalid_tool"));
    Ok(())
}

#[tokio::test]
async fn process_tools_take_only_the_process_they_named() -> TestResult {
    use ma2a_agent::tools::{ProcessStartArgs, ProcessTerminate};
    use ma2a_core::service::{
        FrameKind, LogWindow, ProcessAccept, ProcessDetails, ProcessId, ProcessReply,
        ProcessSummary,
    };
    let (a, b) = (
        ProcessId::from_bytes([1; 16]),
        ProcessId::from_bytes([2; 16]),
    );
    let process = |id| -> TestResult<ProcessSummary> {
        Ok(ProcessSummary::new(id, RequestId::random()?, 1_000))
    };
    let answer = |accept: ProcessAccept, reply: ProcessReply| -> TestResult<Script> {
        Ok(vec![
            (FrameKind::Accept, accept.encode()?),
            (
                FrameKind::Outcome,
                ProcessReply::encode_outcome(&Ok(reply))?,
            ),
        ])
    };
    let details = |id| -> TestResult<ProcessReply> {
        let details = ProcessDetails::new(process(id)?, None, LogWindow::new(0, 0));
        Ok(ProcessReply::Details(Box::new(details)))
    };
    let endpoint = target();
    let inspect = ProcessTarget {
        endpoint: endpoint.clone(),
        id: a.to_string(),
    };
    let terminate = ProcessTerminate {
        endpoint: endpoint.clone(),
        id: a.to_string(),
        grace_ms: None,
    };
    let start = ProcessStartArgs {
        endpoint,
        program: "true".to_owned(),
        args: Vec::new(),
        cwd: None,
        env: std::collections::BTreeMap::new(),
        name: None,
    };

    let own = answer(ProcessAccept::Admitted, details(a)?)?;
    let inspected = answered_code("inspect-own", own, async |call| {
        call.process_inspect(inspect.clone()).await
    });
    assert_eq!(inspected.await?, None);
    for (case, reply) in [
        ("inspect-other", details(b)?),
        ("inspect-summary", ProcessReply::Summary(process(a)?)),
    ] {
        let script = answer(ProcessAccept::Admitted, reply)?;
        let inspected = answered_code(case, script, async |call| {
            call.process_inspect(inspect.clone()).await
        });
        assert_eq!(inspected.await?.as_deref(), Some("invalid_tool"), "{case}");
    }
    for (case, reply) in [
        ("terminate-other", ProcessReply::Summary(process(b)?)),
        ("terminate-details", details(a)?),
    ] {
        let script = answer(ProcessAccept::Admitted, reply)?;
        let terminated = answered_code(case, script, async |call| {
            call.process_terminate(terminate.clone()).await
        });
        assert_eq!(terminated.await?.as_deref(), Some("invalid_tool"), "{case}");
    }
    let script = answer(
        ProcessAccept::Started(b, false),
        ProcessReply::Summary(process(a)?),
    )?;
    let started = answered_code("start-other", script, async |call| {
        call.process_start(start.clone()).await
    });
    assert_eq!(started.await?.as_deref(), Some("invalid_tool"));
    Ok(())
}
