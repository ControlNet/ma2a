//! Real daemons joined through the installed CLI, with services exposed and granted.

#![allow(
    dead_code,
    reason = "the Phase 2 and Phase 3 acceptance binaries use different parts of the lab"
)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::Value;

use super::daemon_fixture::{DaemonFixture, TestResult, TestValue};

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// One Endpoint: a foreground daemon this test owns, and its identity.
pub(crate) struct Node {
    fixture: DaemonFixture,
    id: String,
}

impl Node {
    pub(crate) fn start(label: &str) -> TestValue<Self> {
        let fixture = DaemonFixture::running(&format!("services-{label}"))?;
        let shown = fixture.json(&["endpoint", "show", "--json"])?;
        let id = text(&shown, "/result/payload/endpoint_id");
        if id.len() != 64 {
            return Err(format!("endpoint show returned no identity: {shown}").into());
        }
        Ok(Self { fixture, id })
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    pub(crate) fn state_dir(&self) -> &Path {
        self.fixture.state_dir()
    }

    pub(crate) fn command(&self, arguments: &[&str]) -> Command {
        self.fixture.command(arguments)
    }

    pub(crate) fn run(&self, arguments: &[&str]) -> TestValue<Output> {
        self.fixture.run(arguments)
    }

    pub(crate) fn run_ok(&self, arguments: &[&str]) -> TestValue<Output> {
        self.fixture.run_ok(arguments)
    }

    pub(crate) fn json(&self, arguments: &[&str]) -> TestValue<Value> {
        self.fixture.json(arguments)
    }

    /// Runs one command with `input` on stdin.
    pub(crate) fn piped(&self, arguments: &[&str], input: &[u8]) -> TestValue<Output> {
        use std::io::Write as _;

        let mut child = self.command(arguments).stdin(Stdio::piped()).spawn()?;
        let mut stdin = child.stdin.take().ok_or("missing stdin")?;
        let input = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let output = child.wait_with_output()?;
        writer.join().map_err(|_| "stdin writer panicked")??;
        Ok(output)
    }

    /// Stops the daemon and starts it again on the same state directory.
    pub(crate) fn restart(&mut self) -> TestResult {
        self.fixture.stop_owned()?;
        self.fixture.start_owned()
    }

    /// Kills the daemon as a crash would, then starts it again with `environment`.
    pub(crate) fn crash_restart(&mut self, environment: &[(&str, &str)]) -> TestResult {
        self.fixture.kill_owned()?;
        self.fixture.start_owned_with(environment)
    }

    /// Restarts the daemon with `environment`, such as a fault point it will abort at.
    pub(crate) fn restart_with(&mut self, environment: &[(&str, &str)]) -> TestResult {
        self.fixture.stop_owned()?;
        self.fixture.start_owned_with(environment)
    }

    /// Waits for a daemon that aborts by itself at a fault point, then starts a clean one.
    pub(crate) fn recover_from_fault(&mut self) -> TestResult {
        self.fixture
            .await_owned_exit(std::time::Duration::from_secs(30))?;
        self.fixture.start_owned()
    }

    /// Returns the process identifier of this Endpoint's daemon.
    pub(crate) fn daemon_pid(&self) -> Option<u32> {
        self.fixture.owned_pid()
    }

    /// Returns the `active_count` of this Runtime's operations.
    pub(crate) fn active_operations(&self) -> TestValue<u64> {
        let operations = self.json(&["ops", "--json"])?;
        operations
            .pointer("/result/payload/active_count")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("ops returned no active count: {operations}").into())
    }

    pub(crate) fn shutdown(self) -> TestResult {
        self.fixture.shutdown()
    }
}

/// Creates Space `name` on `owner` and joins `member` through the piped invite.
pub(crate) fn join(owner: &Node, member: &Node, name: &str) -> TestValue<String> {
    let listed = owner.json(&["space", "list", "--json"])?;
    let known = listed
        .pointer("/result/payload")
        .and_then(Value::as_array)
        .and_then(|spaces| {
            spaces
                .iter()
                .find(|space| space.get("name").and_then(Value::as_str) == Some(name))
                .map(|space| text(space, "/space_id"))
        });
    let space_id = match known {
        Some(space_id) => space_id,
        None => text(
            &owner.json(&["space", "create", name, "--json"])?,
            "/result/payload/space_id",
        ),
    };
    let mut invite = owner
        .command(&["space", "invite", &space_id])
        .stdout(Stdio::piped())
        .spawn()?;
    let ticket = invite.stdout.take().ok_or("missing invite stdout")?;
    let accepted = member
        .command(&["space", "accept"])
        .stdin(Stdio::from(ticket))
        .output()?;
    succeed(&invite.wait_with_output()?)?;
    succeed(&accepted)?;
    Ok(space_id)
}

/// A private directory exposed as a Files root; removed on drop.
pub(crate) struct Root(PathBuf);

impl Root {
    pub(crate) fn new(label: &str) -> TestValue<Self> {
        let serial = NEXT_ROOT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-services-root-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn as_arg(&self) -> TestValue<&str> {
        self.0
            .to_str()
            .ok_or_else(|| "root path is not UTF-8".into())
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

/// The Phase 2 lab: `owner` serves every service to Space `lab`, where `member` holds
/// every grant.
pub(crate) struct Lab {
    pub(crate) owner: Node,
    pub(crate) member: Node,
    pub(crate) space: String,
    pub(crate) root: Root,
}

impl Lab {
    pub(crate) fn start(label: &str, proxy_ports: &str) -> TestValue<Self> {
        let owner = Node::start(&format!("{label}-owner"))?;
        let member = Node::start(&format!("{label}-member"))?;
        let space = join(&owner, &member, "lab")?;
        let root = Root::new(label)?;
        owner.run_ok(&["service", "expose", "files", &space, "docs", root.as_arg()?])?;
        owner.run_ok(&["service", "expose", "exec", &space])?;
        owner.run_ok(&["service", "expose", "shell", &space])?;
        owner.run_ok(&[
            "service",
            "expose",
            "proxy",
            &space,
            "127.0.0.1",
            proxy_ports,
        ])?;
        owner.run_ok(&[
            "space",
            "grant",
            &space,
            member.id(),
            "--files-read",
            "--files-write",
            "--exec",
            "--shell",
            "--proxy",
        ])?;
        Ok(Self {
            owner,
            member,
            space,
            root,
        })
    }

    /// Runs a remote-service command from the member against the owner.
    pub(crate) fn remote(&self, command: &[&str], rest: &[&str]) -> TestValue<Output> {
        let mut arguments = command.to_vec();
        arguments.extend(["--endpoint", self.owner.id()]);
        arguments.extend(rest);
        self.member.run(&arguments)
    }

    pub(crate) fn shutdown(self) -> TestResult {
        let member = self.member.shutdown();
        self.owner.shutdown()?;
        member
    }
}

pub(crate) fn succeed(output: &Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned().into())
    }
}

/// Asserts a command failed and its diagnostics name `expected`.
pub(crate) fn refused(output: &Output, expected: &str) -> TestResult {
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("refusal: {}", stderr.trim());
    if output.status.success() || !stderr.contains(expected) {
        return Err(format!("expected a refusal naming {expected:?}, got: {stderr}").into());
    }
    Ok(())
}

pub(crate) fn text(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Deterministic pseudo-random content of `length` bytes.
pub(crate) fn content(length: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}
