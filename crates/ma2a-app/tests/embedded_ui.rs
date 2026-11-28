//! Production-binary embedded Web acceptance coverage.

#![cfg(target_os = "linux")]

#[path = "embedded_ui/http.rs"]
mod http;

use std::{io::Write as _, path::Path, process::Stdio};

use serde_json::Value;

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;

use daemon_fixture::{DaemonFixture, TestResult, TestValue};

const PASSWORD: &str = "embedded-ui-process-password";

struct Fixture {
    daemon: DaemonFixture,
    port: u16,
}

impl Fixture {
    fn start() -> TestValue<Self> {
        let daemon = DaemonFixture::running("embedded-ui")?;
        let port = start_web_ui(daemon.state_dir())?;
        Ok(Self { daemon, port })
    }

    fn shutdown(self) -> TestResult {
        self.daemon.shutdown()
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
        .map_err(|error| format!("embedded GET {path}: {error}").into())
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
        .map_err(|error| format!("embedded POST {path}: {error}").into())
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
    let csrf_cookie_name = csrf_cookie
        .split_once('=')
        .map(|(name, _)| name)
        .ok_or("malformed CSRF cookie")?;
    assert!(
        index.contains(&format!(
            "<meta name=\"ma2a-csrf-cookie\" content=\"{csrf_cookie_name}\" />"
        )),
        "index.html did not expose the matching CSRF cookie name"
    );
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
    fixture.shutdown()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_runtimes_on_same_host_keep_independent_sessions() -> TestResult {
    let first = Fixture::start()?;
    let second = Fixture::start()?;
    let mut jar = std::collections::BTreeMap::<String, String>::new();

    let first_html = first.get("/login", None)?;
    let second_html = second.get("/login", None)?;
    let first_meta = csrf_cookie_meta(std::str::from_utf8(&first_html.body)?)?;
    let second_meta = csrf_cookie_meta(std::str::from_utf8(&second_html.body)?)?;
    assert_ne!(first_meta, second_meta);

    let login_body = serde_json::json!({"password": PASSWORD}).to_string();
    let first_login = first.post("/api/v1/web/auth/login", (&login_body, ""))?;
    let second_login = second.post("/api/v1/web/auth/login", (&login_body, ""))?;
    assert_eq!(first_login.status, 200);
    assert_eq!(second_login.status, 200);
    apply_set_cookies(&mut jar, first_login.headers("set-cookie"));
    apply_set_cookies(&mut jar, second_login.headers("set-cookie"));
    let second_csrf = jar
        .get(second_meta)
        .ok_or("shared host jar lost second runtime CSRF cookie")?
        .clone();

    let first_snapshot = first.get("/api/v1/snapshot", Some(&jar_header(&jar)))?;
    let second_snapshot = second.get("/api/v1/snapshot", Some(&jar_header(&jar)))?;
    assert_eq!(first_snapshot.status, 200);
    assert_eq!(second_snapshot.status, 200);
    let first_body: Value = serde_json::from_slice(&first_snapshot.body)?;
    let second_body: Value = serde_json::from_slice(&second_snapshot.body)?;
    assert_ne!(
        first_body.pointer("/endpoint/endpoint_id"),
        second_body.pointer("/endpoint/endpoint_id")
    );

    let logout_headers = format!(
        "Cookie: {}\r\nX-CSRF-Token: {second_csrf}\r\n",
        jar_header(&jar)
    );
    let second_logout = second.post("/api/v1/web/auth/logout", ("", &logout_headers))?;
    assert_eq!(second_logout.status, 204);
    apply_set_cookies(&mut jar, second_logout.headers("set-cookie"));
    assert!(jar.contains_key(first_meta));
    assert!(!jar.contains_key(second_meta));

    assert_eq!(
        first
            .get("/api/v1/snapshot", Some(&jar_header(&jar)))?
            .status,
        200
    );
    assert_eq!(
        second
            .get("/api/v1/snapshot", Some(&jar_header(&jar)))?
            .status,
        401
    );

    first.shutdown()?;
    second.shutdown()
}

fn start_web_ui(state_dir: &Path) -> TestValue<u16> {
    let mut init = ma2a(state_dir, &["ui", "init"])
        .env("MA2A_PASSWORD_STDIN", "1")
        .stdin(Stdio::piped())
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
    let started = ma2a(state_dir, &["ui", "start", "--json"]).output()?;
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

fn ma2a(state_dir: &Path, arguments: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_ma2a"));
    command
        .arg("--state-dir")
        .arg(state_dir)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
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
    let session_name = cookie_header
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| name.starts_with("ma2a_session"))
        .map(|(name, _)| name)
        .ok_or("session cookie missing from header")?;
    let csrf_name = cookie_header
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| name.starts_with("ma2a_csrf"))
        .map(|(name, _)| name)
        .ok_or("CSRF cookie missing from header")?;
    assert!(response.headers("set-cookie").iter().any(|cookie| {
        cookie == &format!("{session_name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
    }));
    assert!(
        response.headers("set-cookie").iter().any(|cookie| {
            cookie == &format!("{csrf_name}=; Path=/; SameSite=Strict; Max-Age=0")
        })
    );
    assert_eq!(
        fixture.get("/api/v1/snapshot", Some(cookie_header))?.status,
        401
    );
    Ok(())
}

fn csrf_cookie_meta(html: &str) -> TestValue<&str> {
    let marker = "<meta name=\"ma2a-csrf-cookie\" content=\"";
    let rest = html
        .split_once(marker)
        .ok_or("ma2a-csrf-cookie meta missing")?
        .1;
    rest.split_once('"')
        .map(|(value, _)| value)
        .ok_or_else(|| "ma2a-csrf-cookie meta unterminated".into())
}

fn apply_set_cookies(jar: &mut std::collections::BTreeMap<String, String>, headers: &[String]) {
    for raw in headers {
        let Some((pair, attrs)) = raw.split_once(';') else {
            continue;
        };
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        if attrs.contains("Max-Age=0") || value.is_empty() {
            jar.remove(name.trim());
        } else {
            jar.insert(name.trim().to_owned(), value.trim().to_owned());
        }
    }
}

fn jar_header(jar: &std::collections::BTreeMap<String, String>) -> String {
    jar.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
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
    let prefix = format!("{name}_");
    headers
        .iter()
        .filter_map(|value| value.split_once(';').map(|(pair, _)| pair))
        .find(|pair| {
            pair.split_once('=').is_some_and(|(cookie_name, _)| {
                cookie_name == name || cookie_name.starts_with(&prefix)
            })
        })
        .ok_or_else(|| format!("login response has no {name} cookie").into())
}
