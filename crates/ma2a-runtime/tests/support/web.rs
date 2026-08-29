use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
};

use ma2a_runtime::web::{Clock, LoopbackWebServer, WebAssets, WebServerConfig};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    sync::oneshot,
    task::JoinHandle,
};

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempState {
    path: PathBuf,
}

impl TempState {
    pub(crate) fn new(name: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-web-{name}-{}-{serial}", std::process::id()));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _result = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
pub(crate) struct ManualClock {
    now_ms: AtomicI64,
}

impl ManualClock {
    pub(crate) const fn new(now_ms: i64) -> Self {
        Self {
            now_ms: AtomicI64::new(now_ms),
        }
    }

    #[allow(
        dead_code,
        reason = "shared support method is used by the auth test binary"
    )]
    pub(crate) fn set(&self, now_ms: i64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

#[allow(
    dead_code,
    reason = "shared password fixture is used by sibling integration test binaries"
)]
pub(crate) fn password() -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(format!("{}9!z", "A".repeat(16)))
}

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "shared response fixture is used by sibling integration test binaries"
)]
pub(crate) struct HttpResponse {
    pub(crate) status: u16,
    pub(crate) set_cookies: Vec<String>,
    #[allow(
        dead_code,
        reason = "shared response field is used by the security test binary"
    )]
    pub(crate) headers: BTreeMap<String, String>,
    #[allow(
        dead_code,
        reason = "shared response field is used by the auth test binary"
    )]
    pub(crate) body: Vec<u8>,
}

pub(crate) struct RunningServer {
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<std::io::Result<()>>,
}

impl RunningServer {
    pub(crate) async fn start(
        auth: ma2a_runtime::web::WebAuthService,
        config: WebServerConfig,
    ) -> TestResult<Self> {
        let assets = WebAssets::new(&[
            (
                "index.html",
                b"<!doctype html><main id=main-content>MA2A</main>",
            ),
            ("assets/app.js", b"export {}"),
        ]);
        let server = LoopbackWebServer::bind(auth, assets, config).await?;
        let port = server.port();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(server.serve(async move {
            let _result = shutdown_rx.await;
        }));
        Ok(Self {
            port,
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    pub(crate) const fn port(&self) -> u16 {
        self.port
    }

    #[allow(
        dead_code,
        reason = "shared HTTP client is used by sibling integration test binaries"
    )]
    pub(crate) async fn request(&self, request: &[u8]) -> TestResult<HttpResponse> {
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", self.port)).await?;
        stream.write_all(request).await?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await?;
        parse_response(&response)
    }

    pub(crate) async fn stop(mut self) -> TestResult {
        if let Some(shutdown) = self.shutdown.take() {
            let _result = shutdown.send(());
        }
        self.task.await??;
        Ok(())
    }
}

#[allow(
    dead_code,
    reason = "shared HTTP parser is used by sibling integration test binaries"
)]
fn parse_response(response: &[u8]) -> TestResult<HttpResponse> {
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("HTTP response has no header terminator")?;
    let head = std::str::from_utf8(
        response
            .get(..split)
            .ok_or("HTTP response header split is invalid")?,
    )?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or("HTTP response has no status")?
        .parse::<u16>()?;
    let headers = lines
        .clone()
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let set_cookies = lines
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _value)| name.eq_ignore_ascii_case("set-cookie"))
        .map(|(_name, value)| value.trim().to_owned())
        .collect();
    Ok(HttpResponse {
        status,
        set_cookies,
        headers,
        body: response
            .get(split + 4..)
            .ok_or("HTTP response body split is invalid")?
            .to_vec(),
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "the raw HTTP fixture keeps method, target, headers, and body explicit"
)]
#[allow(
    dead_code,
    reason = "shared request fixture is used by sibling integration test binaries"
)]
pub(crate) fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> Vec<u8> {
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("\r\n");
    let mut bytes = request.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

pub(crate) fn clock(value: i64) -> Arc<ManualClock> {
    Arc::new(ManualClock::new(value))
}
