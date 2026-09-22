//! Production-binary embedded Web acceptance coverage.

#![cfg(target_os = "linux")]

#[path = "embedded_ui/http.rs"]
mod http;

use std::{
    error::Error,
    fs,
    io::Write as _,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);
const PASSWORD: &str = "embedded-ui-process-password";

struct Fixture {
    state_dir: PathBuf,
    port: u16,
}

impl Fixture {
    fn start() -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let state_dir =
            std::env::temp_dir().join(format!("ma2a-embedded-ui-{}-{serial}", std::process::id()));
        fs::create_dir(&state_dir)?;
        fs::set_permissions(&state_dir, fs::Permissions::from_mode(0o700))?;
        let started = Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .arg("--state-dir")
            .arg(&state_dir)
            .arg("start")
            .output()?;
        if !started.status.success() {
            let _cleanup_result = fs::remove_dir_all(&state_dir);
            return Err(format!(
                "daemon start failed: {}",
                String::from_utf8_lossy(&started.stderr)
            )
            .into());
        }
        let port = match start_web_ui(&state_dir) {
            Ok(port) => port,
            Err(error) => {
                let _stop_result = Command::new(env!("CARGO_BIN_EXE_ma2a"))
                    .arg("--state-dir")
                    .arg(&state_dir)
                    .arg("stop")
                    .output();
                let _cleanup_result = fs::remove_dir_all(&state_dir);
                return Err(error);
            }
        };
        Ok(Self { state_dir, port })
    }

    fn get(&self, path: &str, cookie: Option<&str>) -> TestValue<http::HttpResponse> {
        let cookie = cookie.map_or(String::new(), |value| format!("Cookie: {value}\r\n"));
        http::request(
            self.port,
            format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{cookie}Connection: close\r\n\r\n",
                self.port
            )
            .as_bytes(),
        )
    }

    fn post(&self, path: &str, request: (&str, &str)) -> TestValue<http::HttpResponse> {
        let (body, headers) = request;
        http::request(
            self.port,
            format!(
                "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nSec-Fetch-Site: same-origin\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                self.port,
                self.port,
                body.len()
            )
            .as_bytes(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .arg("--state-dir")
            .arg(&self.state_dir)
            .arg("stop")
            .output();
        let _cleanup = fs::remove_dir_all(&self.state_dir);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_binary_serves_authenticated_embedded_console() -> TestResult {
    // Given
    let fixture = Fixture::start()?;

    // When
    let login_page = fixture.get("/login", None)?;
    let setup = fixture.get("/setup", None)?;
    let redirected = fixture.get("/", None)?;
    let auth_state = fixture.get("/api/v1/web/auth/state", None)?;
    let protected = fixture.get("/api/v1/snapshot", None)?;

    // Then
    assert_eq!(login_page.status, 200);
    assert_html_security(&login_page)?;
    assert_eq!(login_page.header("cache-control")?, "no-cache");
    assert_eq!(setup.status, 307);
    assert_eq!(setup.header("location")?, "/login");
    assert_eq!(redirected.status, 307);
    assert_eq!(redirected.header("location")?, "/login");
    assert_eq!(protected.status, 401);
    assert_eq!(protected.header("cache-control")?, "no-store");
    let state: Value = serde_json::from_slice(&auth_state.body)?;
    assert_eq!(
        state.get("state").and_then(Value::as_str),
        Some("configured")
    );

    let index = std::str::from_utf8(&login_page.body)?;
    let script = embedded_asset(index, "<script type=\"module\" crossorigin src=\"/")?;
    let style = embedded_asset(index, "<link rel=\"stylesheet\" crossorigin href=\"/")?;
    assert!(
        script.starts_with("assets/")
            && Path::new(script)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("js"))
    );
    assert!(
        style.starts_with("assets/")
            && Path::new(style)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("css"))
    );
    let javascript = fixture.get(&format!("/{script}"), None)?;
    let stylesheet = fixture.get(&format!("/{style}"), None)?;
    assert_eq!(
        javascript.header("cache-control")?,
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        stylesheet.header("cache-control")?,
        "public, max-age=31536000, immutable"
    );
    assert!(!javascript.body.is_empty() && !stylesheet.body.is_empty());
    let missing = fixture.get("/assets/missing-deadbeef.js", None)?;
    assert_eq!(missing.status, 404);
    assert_eq!(missing.header("cache-control")?, "no-store");

    let login = fixture.post(
        "/api/v1/web/auth/login",
        (&serde_json::json!({"password": PASSWORD}).to_string(), ""),
    )?;
    assert_eq!(login.status, 200);
    let login_body: Value = serde_json::from_slice(&login.body)?;
    let csrf = login_body
        .get("csrf_token")
        .and_then(Value::as_str)
        .ok_or("login response has no CSRF token")?;
    let cookies = login.headers("set-cookie");
    let session = cookie_pair(cookies, "ma2a_session")?;
    let csrf_cookie = cookie_pair(cookies, "ma2a_csrf")?;
    let cookie_header = format!("{session}; {csrf_cookie}");
    let snapshot = fixture.get("/api/v1/snapshot", Some(&cookie_header))?;
    assert_eq!(snapshot.status, 200);
    assert_eq!(snapshot.header("cache-control")?, "no-store");
    let snapshot_body: Value = serde_json::from_slice(&snapshot.body)?;
    assert!(snapshot_body.get("endpoint").is_some());
    let deep_route = fixture.get("/spaces", Some(&cookie_header))?;
    assert_eq!(deep_route.status, 200);
    assert_eq!(deep_route.header("cache-control")?, "no-cache");
    assert_eq!(deep_route.body, login_page.body);
    assert_revoke_all_signs_out(&fixture, &cookie_header, csrf)?;
    Ok(())
}

fn start_web_ui(state_dir: &Path) -> TestValue<u16> {
    let mut init = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(state_dir)
        .args(["ui", "init"])
        .env("MA2A_PASSWORD_STDIN", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    init.stdin
        .take()
        .ok_or("ui init stdin is unavailable")?
        .write_all(format!("{PASSWORD}\n{PASSWORD}\n").as_bytes())?;
    let initialized = init.wait_with_output()?;
    if !initialized.status.success() {
        return Err(format!(
            "ui init failed: {}",
            String::from_utf8_lossy(&initialized.stderr)
        )
        .into());
    }
    let started = Command::new(env!("CARGO_BIN_EXE_ma2a"))
        .arg("--state-dir")
        .arg(state_dir)
        .args(["ui", "start", "--json"])
        .output()?;
    if !started.status.success() {
        return Err(format!(
            "ui start failed: {}",
            String::from_utf8_lossy(&started.stderr)
        )
        .into());
    }
    let status: Value = serde_json::from_slice(&started.stdout)?;
    let url = status
        .get("url")
        .and_then(Value::as_str)
        .ok_or("ui start response has no URL")?;
    let (_, port) = url
        .rsplit_once(':')
        .ok_or("ui start response has an invalid URL")?;
    Ok(port.parse()?)
}

fn assert_revoke_all_signs_out(fixture: &Fixture, cookie_header: &str, csrf: &str) -> TestResult {
    let body = serde_json::json!({
        "version": 1,
        "operation": "session_revoke_all",
        "request_id": "00112233445566778899aabbccddeeff"
    })
    .to_string();
    let headers = format!("Cookie: {cookie_header}\r\nX-CSRF-Token: {csrf}\r\n");
    let response = fixture.post("/api/v1/sessions/revoke-all", (&body, &headers))?;
    assert_eq!(response.status, 200);
    assert!(
        response.headers("set-cookie").iter().any(|cookie| {
            cookie == "ma2a_session=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0"
        })
    );
    assert!(
        response
            .headers("set-cookie")
            .iter()
            .any(|cookie| { cookie == "ma2a_csrf=; Path=/; SameSite=Strict; Max-Age=0" })
    );
    assert_eq!(
        fixture.get("/api/v1/snapshot", Some(cookie_header))?.status,
        401
    );
    Ok(())
}

fn assert_html_security(response: &http::HttpResponse) -> TestResult {
    assert_eq!(response.header("content-type")?, "text/html; charset=utf-8");
    assert_eq!(response.header("x-content-type-options")?, "nosniff");
    assert_eq!(response.header("x-frame-options")?, "DENY");
    assert_eq!(response.header("referrer-policy")?, "no-referrer");
    assert!(
        response
            .header("content-security-policy")?
            .contains("default-src 'self'")
    );
    Ok(())
}

fn embedded_asset<'a>(index: &'a str, prefix: &str) -> TestValue<&'a str> {
    let remainder = index
        .split_once(prefix)
        .ok_or("embedded asset reference missing")?
        .1;
    remainder
        .split_once('"')
        .map(|(path, _)| path)
        .ok_or_else(|| "embedded asset reference is unterminated".into())
}

fn cookie_pair<'a>(headers: &'a [String], name: &str) -> TestValue<&'a str> {
    headers
        .iter()
        .filter_map(|value| value.split_once(';').map(|(pair, _)| pair))
        .find(|pair| pair.starts_with(&format!("{name}=")))
        .ok_or_else(|| format!("login response has no {name} cookie").into())
}
