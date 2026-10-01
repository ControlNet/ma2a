//! Two joined daemons: `a` hosts service `example` (at most 2 calls) and exposes it and Exec
//! to Space `eco`, where `b` holds the Service and Exec grants.
//!
//! The service is `ma2a-example-service`, the deterministic example built on the SDK; it is a
//! test and documentation example, not a production service.

use std::{
    fs,
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Node, Root, join, text},
};

/// The service name `a` hosts.
pub(crate) const SERVICE: &str = "example";
/// How many calls `example` serves at once.
pub(crate) const MAX_CALLS: &str = "2";

/// One running `ma2a-example-service`; killed when dropped.
pub(crate) struct Program(Child);

impl Program {
    pub(crate) fn spawn(node: &Node) -> TestValue<Self> {
        let state_dir = node.state_dir().to_str().ok_or("state dir is not UTF-8")?;
        let child = Command::new(example_service()?)
            .args(["serve", state_dir, SERVICE])
            .stdin(Stdio::null())
            .spawn()?;
        Ok(Self(child))
    }

    /// Kills the program as a crash would.
    pub(crate) fn kill(mut self) -> TestResult {
        self.0.kill()?;
        self.0.wait()?;
        Ok(())
    }

    /// Waits for the program to exit by itself; its exit status.
    pub(crate) fn exited(&mut self, deadline: Duration) -> TestValue<std::process::ExitStatus> {
        let until = Instant::now() + deadline;
        loop {
            if let Some(status) = self.0.try_wait()? {
                return Ok(status);
            }
            if Instant::now() > until {
                return Err("the service program did not exit".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

pub(crate) struct EcoLab {
    pub(crate) program: Option<Program>,
    pub(crate) a: Node,
    pub(crate) b: Node,
    _manifest: Root,
}

impl EcoLab {
    pub(crate) fn start(label: &str) -> TestValue<Self> {
        let a = Node::start(&format!("{label}-a"))?;
        let b = Node::start(&format!("{label}-b"))?;
        let space = join(&a, &b, "eco")?;
        let manifest = Root::new(label)?;
        let file = manifest.path().join("manifest.json");
        let printed = Command::new(example_service()?)
            .args(["manifest", MAX_CALLS])
            .output()?;
        if !printed.status.success() {
            return Err("ma2a-example-service manifest failed".into());
        }
        fs::write(&file, &printed.stdout)?;
        let file = file.to_str().ok_or("manifest path is not UTF-8")?;
        a.run_ok(&["service", "register", SERVICE, "--manifest", file])?;
        a.run_ok(&["service", "start", SERVICE])?;
        a.run_ok(&["service", "expose", "service", &space, SERVICE])?;
        a.run_ok(&["service", "expose", "exec", &space])?;
        a.run_ok(&["space", "grant", &space, b.id(), "--service", "--exec"])?;
        let program = Program::spawn(&a)?;
        wait_state(&a, "ready")?;
        Ok(Self {
            program: Some(program),
            a,
            b,
            _manifest: manifest,
        })
    }

    /// Runs `ma2a service call` from `b` against `a`'s `example`.
    pub(crate) fn call(&self, rest: &[&str]) -> TestValue<Output> {
        self.b.run(&call_arguments(self.a.id(), rest))
    }

    /// The same, with `input` on stdin.
    pub(crate) fn piped(&self, rest: &[&str], input: &[u8]) -> TestValue<Output> {
        self.b.piped(&call_arguments(self.a.id(), rest), input)
    }

    /// Starts one call from `b` in the background.
    pub(crate) fn spawn_call(&self, rest: &[&str]) -> TestValue<Child> {
        let arguments = call_arguments(self.a.id(), rest);
        Ok(self
            .b
            .command(&arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?)
    }

    /// `b`'s discovery of `a`.
    pub(crate) fn discover(&self) -> TestValue<Value> {
        let output = self
            .b
            .run(&["service", "discover", "--endpoint", self.a.id()])?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    pub(crate) fn shutdown(mut self) -> TestResult {
        drop(self.program.take());
        let b = self.b.shutdown();
        self.a.shutdown()?;
        b
    }
}

pub(crate) fn call_arguments<'a>(endpoint: &'a str, rest: &[&'a str]) -> Vec<&'a str> {
    let mut arguments = vec!["service", "call", "--endpoint", endpoint];
    arguments.extend(rest);
    arguments
}

/// `node`'s view of `example`: state, running calls, and per-version call counts.
pub(crate) fn hosted(node: &Node) -> TestValue<Value> {
    let listed = node.json(&["service", "hosted", "--json"])?;
    listed
        .pointer("/result/payload/services")
        .and_then(Value::as_array)
        .and_then(|services| {
            services
                .iter()
                .find(|service| text(service, "/service") == SERVICE)
                .cloned()
        })
        .ok_or_else(|| format!("{SERVICE} is not registered: {listed}").into())
}

/// Waits until `example` on `node` is in `state`.
pub(crate) fn wait_state(node: &Node, state: &str) -> TestResult {
    wait_for(node, |view| text(view, "/state") == state)
}

/// Waits until `example` on `node` runs exactly `count` calls.
pub(crate) fn wait_running(node: &Node, count: u64) -> TestResult {
    wait_for(node, |view| {
        view.get("running_calls").and_then(Value::as_u64) == Some(count)
    })
}

fn wait_for(node: &Node, done: impl Fn(&Value) -> bool) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let view = hosted(node)?;
        if done(&view) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("{SERVICE} never reached the expected state: {view}").into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Sends SIGINT, as Ctrl-C would, to one background call.
pub(crate) fn interrupt(child: &Child) -> TestResult {
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err("kill -INT failed".into())
    }
}

/// Returns `ma2a-example-service` beside the `ma2a` under test, building it when absent.
///
/// Building it from here unconditionally would rebuild shared dependencies with this package's
/// feature set on every run; `cargo build --workspace --bins` refreshes it after a change.
pub(crate) fn example_service() -> TestValue<String> {
    static BUILT: OnceLock<Result<String, String>> = OnceLock::new();
    BUILT
        .get_or_init(|| locate().map_err(|error| error.to_string()))
        .clone()
        .map_err(Into::into)
}

fn locate() -> TestValue<String> {
    let ma2a = Path::new(env!("CARGO_BIN_EXE_ma2a"));
    let name = format!("ma2a-example-service{}", std::env::consts::EXE_SUFFIX);
    let path = ma2a.with_file_name(name);
    if !path.exists() {
        let profile_dir = ma2a.parent().ok_or("ma2a has no directory")?;
        let target_dir = profile_dir.parent().ok_or("no target directory")?;
        let profile = match profile_dir.file_name().and_then(|name| name.to_str()) {
            Some("debug") => "dev",
            Some(other) => other,
            None => return Err("unnamed profile directory".into()),
        };
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        let built = Command::new(cargo)
            .args([
                "build",
                "-p",
                "ma2a-service",
                "--bin",
                "ma2a-example-service",
            ])
            .args(["--profile", profile])
            .env("CARGO_TARGET_DIR", target_dir)
            .status()?;
        if !built.success() || !path.exists() {
            return Err("could not build ma2a-example-service".into());
        }
    }
    path.into_os_string()
        .into_string()
        .map_err(|_| "service path is not UTF-8".into())
}
