//! Installed-CLI coverage for the piped invite/accept and leave workflows.

use std::{
    error::Error,
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;
type TestValue<T> = Result<T, Box<dyn Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct Endpoint(PathBuf);

impl Endpoint {
    fn start(label: &str) -> TestValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-cli-membership-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        let endpoint = Self(path);
        succeed(&endpoint.run(&["start"])?)?;
        Ok(endpoint)
    }

    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ma2a"));
        command.arg("--state-dir").arg(&self.0).args(arguments);
        command
    }

    fn run(&self, arguments: &[&str]) -> Result<Output, std::io::Error> {
        self.command(arguments).output()
    }

    fn spaces(&self) -> TestValue<Vec<Value>> {
        let listed = self.run(&["space", "list", "--json"])?;
        succeed(&listed)?;
        Ok(serde_json::from_slice::<Value>(&listed.stdout)?
            .pointer("/result/payload")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _shutdown = self.run(&["stop"]);
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

fn succeed(output: &Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_piped_invite_joins_a_named_space_and_a_member_can_leave_it() -> TestResult {
    // Given
    let owner = Endpoint::start("owner")?;
    let member = Endpoint::start("member")?;
    let created = owner.run(&["space", "create", "lab", "--json"])?;
    succeed(&created)?;
    let space_id = text(
        &serde_json::from_slice::<Value>(&created.stdout)?,
        "/result/payload/space_id",
    );

    // When: the documented pipeline joins without any flag or file.
    let mut invite = owner
        .command(&["space", "invite", "lab"])
        .stdout(Stdio::piped())
        .spawn()?;
    let ticket = invite.stdout.take().ok_or("missing invite stdout")?;
    let accepted = member
        .command(&["space", "accept"])
        .stdin(Stdio::from(ticket))
        .output()?;
    succeed(&invite.wait_with_output()?)?;

    // Then: the joining Endpoint knows the shared Space name it never configured.
    succeed(&accepted)?;
    let joined = member.spaces()?;
    let [joined] = joined.as_slice() else {
        return Err("the member must belong to exactly one Space".into());
    };
    assert_eq!(text(joined, "/name"), "lab");
    assert_eq!(text(joined, "/space_id"), space_id);
    let shown = member.run(&["space", "show", "lab"])?;
    succeed(&shown)?;
    assert!(String::from_utf8(shown.stdout)?.contains("Name: lab"));

    // The owner cannot leave, and a member's departure is authority-signed.
    let refused = owner.run(&["space", "leave", "lab"])?;
    assert!(!refused.status.success());
    assert!(String::from_utf8(refused.stderr)?.contains("Space owner cannot leave its own Space"));
    let left = member.run(&["space", "leave", "lab", "--json"])?;
    succeed(&left)?;
    assert_eq!(
        text(
            &serde_json::from_slice::<Value>(&left.stdout)?,
            "/result/type"
        ),
        "space_left"
    );
    assert!(member.spaces()?.is_empty());
    let owner_spaces = owner.spaces()?;
    let [remaining] = owner_spaces.as_slice() else {
        return Err("the owner must still belong to exactly one Space".into());
    };
    assert_eq!(
        remaining.pointer("/member_count").and_then(Value::as_u64),
        Some(1),
        "the owner's signed membership must shrink when a member leaves"
    );

    // A departed Endpoint rejoins through a fresh invite.
    let mut rejoin = owner
        .command(&["space", "invite", "lab", "--ttl", "60s"])
        .stdout(Stdio::piped())
        .spawn()?;
    let ticket = rejoin.stdout.take().ok_or("missing invite stdout")?;
    let rejoined = member
        .command(&["space", "accept"])
        .stdin(Stdio::from(ticket))
        .output()?;
    succeed(&rejoin.wait_with_output()?)?;
    succeed(&rejoined)?;
    assert_eq!(member.spaces()?.len(), 1);
    Ok(())
}
