//! Process-level coverage for invite, relay, and daemon-owned UI workflows.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_store::{Repository, StoreConfig};
use serde_json::Value;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-cli-workflows-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }

    fn run(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        Command::new(env!("CARGO_BIN_EXE_ma2a"))
            .arg("--state-dir")
            .arg(&self.0)
            .args(arguments)
            .output()
    }

    fn create_space(&self) -> TestValue<String> {
        self.create_named_space("Workflows")
    }

    fn create_named_space(&self, name: &str) -> TestValue<String> {
        let _started = self.run(&["start"])?;
        let output = self.run(&["space", "create", name, "--json"])?;
        assert_success(&output)?;
        let response: Value = serde_json::from_slice(&output.stdout)?;
        response
            .pointer("/result/payload/space_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| "missing created Space ID".into())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _shutdown = self.run(&["stop"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invite_prints_only_the_ticket_and_defaults_to_five_minutes() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_space()?;

    // When
    let by_name = fixture.run(&["space", "invite", "Workflows"])?;
    let by_id = fixture.run(&["space", "invite", &space, "--ttl", "30s"])?;

    // Then
    assert_success(&by_name)?;
    assert_success(&by_id)?;
    let ticket = String::from_utf8(by_name.stdout)?;
    assert_eq!(
        ticket.lines().count(),
        1,
        "stdout must carry only the ticket"
    );
    assert!(ticket.starts_with("ma2ainvite"), "{ticket}");
    assert!(String::from_utf8(by_id.stdout)?.starts_with("ma2ainvite"));
    let leftovers = fs::read_dir(&fixture.0)?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(".invite-"))
        .count();
    assert_eq!(leftovers, 0, "the temporary ticket file must be removed");
    let status: Value = serde_json::from_slice(&fixture.run(&["status", "--json"])?.stdout)?;
    let actor_revision = status
        .get("revision")
        .and_then(Value::as_u64)
        .ok_or("missing Runtime revision")?;
    let persisted_revision = Repository::open(&StoreConfig::new(&fixture.0))?.revision()?;
    assert_eq!(actor_revision, persisted_revision);
    Ok(())
}

#[test]
fn invite_rejects_an_invalid_ttl_and_an_unknown_space() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_space()?;

    // When
    let ttl = fixture.run(&["space", "invite", &space, "--ttl", "6m"])?;
    let unknown = fixture.run(&["space", "invite", "absent"])?;

    // Then
    assert_eq!(ttl.status.code(), Some(2));
    assert!(!unknown.status.success());
    assert!(String::from_utf8(unknown.stderr)?.contains("no Space named"));
    Ok(())
}

#[test]
fn duplicate_space_names_resolve_by_identifier_and_fail_as_ambiguous_by_name() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let first = fixture.create_named_space("lab")?;
    let unique = fixture.create_named_space("unique")?;

    // When
    let single = fixture.run(&["space", "show", "lab", "--json"])?;
    let second = fixture.create_named_space("lab")?;
    let ambiguous = fixture.run(&["space", "show", "lab"])?;
    let by_id = fixture.run(&["space", "show", &second, "--json"])?;
    let other = fixture.run(&["space", "show", "unique", "--json"])?;

    // Then
    assert_success(&single)?;
    assert_eq!(
        serde_json::from_slice::<Value>(&single.stdout)?
            .pointer("/result/payload/space_id")
            .and_then(Value::as_str),
        Some(first.as_str())
    );
    assert_ne!(first, second);
    assert_eq!(ambiguous.status.code(), Some(2));
    let stderr = String::from_utf8(ambiguous.stderr)?;
    assert!(
        stderr.contains(&first) && stderr.contains(&second),
        "{stderr}"
    );
    assert!(stderr.contains("use the Space ID instead"), "{stderr}");
    assert_eq!(
        serde_json::from_slice::<Value>(&by_id.stdout)?
            .pointer("/result/payload/space_id")
            .and_then(Value::as_str),
        Some(second.as_str())
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&other.stdout)?
            .pointer("/result/payload/space_id")
            .and_then(Value::as_str),
        Some(unique.as_str())
    );
    Ok(())
}

#[test]
fn human_space_output_names_the_space_and_its_identifier() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_named_space("lab")?;

    // When
    let shown = fixture.run(&["space", "show", "lab"])?;

    // Then
    assert_success(&shown)?;
    let stdout = String::from_utf8(shown.stdout)?;
    assert!(stdout.contains("Name: lab"), "{stdout}");
    assert!(stdout.contains(&format!("Space ID: {space}")), "{stdout}");
    Ok(())
}

#[test]
fn a_runtime_error_envelope_reports_its_real_protocol_error() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let _space = fixture.create_space()?;
    let absent = "0".repeat(64);

    // When
    let shown = fixture.run(&["space", "show", &absent])?;

    // Then
    assert!(!shown.status.success());
    let stderr = String::from_utf8(shown.stderr)?;
    assert!(stderr.contains("not_found"), "{stderr}");
    assert!(!stderr.contains("missing its result type"), "{stderr}");
    Ok(())
}

#[test]
fn the_space_owner_cannot_leave_its_own_space() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let _space = fixture.create_named_space("owned")?;

    // When
    let left = fixture.run(&["space", "leave", "owned"])?;

    // Then
    assert!(!left.status.success());
    let stderr = String::from_utf8(left.stderr)?;
    assert!(
        stderr.contains("Space owner cannot leave its own Space"),
        "{stderr}"
    );
    assert!(stderr.contains("unauthorized"), "{stderr}");
    let listed = fixture.run(&["space", "list", "--json"])?;
    assert_eq!(
        serde_json::from_slice::<Value>(&listed.stdout)?
            .pointer("/result/payload")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1),
        "a refused departure must not remove local membership"
    );
    Ok(())
}

#[test]
fn removed_space_syntax_is_rejected() -> TestResult {
    // Given
    let fixture = Fixture::new()?;

    // When
    let named = fixture.run(&["space", "create", "--name", "Legacy"])?;
    let selector = fixture.run(&["space", "show", "--space", &"0".repeat(64)])?;
    let nested_invite = fixture.run(&["space", "invite", "create", "--space", &"0".repeat(64)])?;
    let redeem = fixture.run(&["space", "invite", "redeem", "--stdin"])?;
    let revoke = fixture.run(&["space", "member", "revoke", "--space", &"0".repeat(64)])?;

    // Then
    for rejected in [named, selector, nested_invite, redeem, revoke] {
        assert_eq!(rejected.status.code(), Some(2));
    }
    Ok(())
}

#[test]
fn external_private_and_public_relays_configure_status_and_disable() -> TestResult {
    // Given
    let fixture = Fixture::new()?;
    let space = fixture.create_space()?;

    // When
    let private = fixture.run(&[
        "relay",
        "private",
        "configure",
        "--listen",
        "127.0.0.1:0",
        "--public-url",
        "https://relay.example",
        "--serve-space",
        &space,
        "--external-tls",
    ])?;
    let public = fixture.run(&[
        "relay",
        "public",
        "configure",
        "--url",
        "https://public.example",
    ])?;

    // Then
    assert_success(&private)?;
    assert_success(&public)?;
    let private_status: Value = serde_json::from_slice(
        &fixture
            .run(&["relay", "private", "status", "--json"])?
            .stdout,
    )?;
    assert_eq!(
        private_status.pointer("/result/payload/configured"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        private_status
            .pointer("/result/payload/mode")
            .and_then(Value::as_str),
        Some("external_termination")
    );
    assert_eq!(
        private_status.pointer("/result/payload/online"),
        Some(&Value::Bool(true))
    );
    assert_ne!(
        private_status.pointer("/result/payload/port"),
        Some(&Value::from(0))
    );
    let public_status: Value = serde_json::from_slice(
        &fixture
            .run(&["relay", "public", "status", "--json"])?
            .stdout,
    )?;
    assert_eq!(
        public_status
            .pointer("/result/payload/url")
            .and_then(Value::as_str),
        Some("https://public.example")
    );
    assert_success(&fixture.run(&["stop"])?)?;
    assert_success(&fixture.run(&["start"])?)?;
    let restored_private: Value = serde_json::from_slice(
        &fixture
            .run(&["relay", "private", "status", "--json"])?
            .stdout,
    )?;
    assert_eq!(
        restored_private.pointer("/result/payload/online"),
        Some(&Value::Bool(true))
    );
    assert_ne!(
        restored_private.pointer("/result/payload/port"),
        Some(&Value::from(0))
    );
    assert_success(&fixture.run(&["relay", "private", "disable"])?)?;
    assert_success(&fixture.run(&["relay", "public", "disable"])?)?;
    Ok(())
}

fn assert_success(output: &Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}
