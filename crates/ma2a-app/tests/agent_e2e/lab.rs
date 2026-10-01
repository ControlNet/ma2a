//! Two joined daemons: `a` hosts Agent `lead` and serves the read-only root `docs`; `b`
//! hosts Agent `worker`. `b` may read `docs`; `a` may use `worker`.
//!
//! Both Agents are `ma2a-example-agent`, the deterministic reference Agent; it is not a
//! language model.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Node, Root, content, join, text},
};

/// Size of the file the `worker` fetches from `docs`.
pub(crate) const INPUT_LEN: usize = 200_000;

/// Stops both Agents when dropped, before the daemons: Agent instances are hosted
/// Processes, which outlive their daemon, so a test that fails early would leak them.
struct AgentsStopped([PathBuf; 2]);

impl Drop for AgentsStopped {
    fn drop(&mut self) {
        for (state_dir, agent) in self.0.iter().zip(["lead", "worker"]) {
            let _stopped = Command::new(env!("CARGO_BIN_EXE_ma2a"))
                .arg("--state-dir")
                .arg(state_dir)
                .args(["agent", "stop", agent])
                .output();
        }
    }
}

pub(crate) struct AgentLab {
    // Declared first, so it drops before the daemons.
    _agents: AgentsStopped,
    pub(crate) a: Node,
    pub(crate) b: Node,
    pub(crate) space: String,
    _root: Root,
}

/// Returns `ma2a-example-agent` beside the `ma2a` under test, building it when absent.
pub(crate) fn example_agent() -> TestValue<String> {
    static BUILT: OnceLock<Result<String, String>> = OnceLock::new();
    BUILT
        .get_or_init(|| locate().map_err(|error| error.to_string()))
        .clone()
        .map_err(Into::into)
}

fn locate() -> TestValue<String> {
    let ma2a = Path::new(env!("CARGO_BIN_EXE_ma2a"));
    let name = format!("ma2a-example-agent{}", std::env::consts::EXE_SUFFIX);
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
            .args(["build", "-p", "ma2a-agent", "--bin", "ma2a-example-agent"])
            .args(["--profile", profile])
            .env("CARGO_TARGET_DIR", target_dir)
            .status()?;
        if !built.success() || !path.exists() {
            return Err("could not build ma2a-example-agent".into());
        }
    }
    path.into_os_string()
        .into_string()
        .map_err(|_| "agent path is not UTF-8".into())
}

/// Every JSON line a command printed.
pub(crate) fn lines(output: &Output) -> TestValue<Vec<Value>> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

/// The last JSON line a command printed.
pub(crate) fn last(output: &Output) -> TestValue<Value> {
    lines(output)?
        .pop()
        .ok_or_else(|| format!("no output: {}", String::from_utf8_lossy(&output.stderr)).into())
}

/// Defines and starts one example Agent on `node`, then waits until it is ready.
pub(crate) fn host(node: &Node, agent: &str, tools: &[&str]) -> TestResult {
    let program = example_agent()?;
    let mut define = vec!["agent", "define", agent, "--max-tasks", "4"];
    for tool in tools {
        define.extend(["--tool", tool]);
    }
    define.extend(["--", program.as_str()]);
    node.run_ok(&define)?;
    node.run_ok(&["agent", "start", agent])?;
    wait_ready(node, agent)
}

/// Waits until `agent` on `node` has a ready instance.
pub(crate) fn wait_ready(node: &Node, agent: &str) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let listed = last(&node.run_ok(&["agent", "list"])?)?;
        let ready = listed
            .pointer("/agents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|entry| {
                text(entry, "/agent") == agent
                    && entry.pointer("/instance/ready") == Some(&Value::Bool(true))
                    && entry.pointer("/instance/ended_at_ms") == Some(&Value::Null)
            });
        if ready {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("{agent} never became ready: {listed}").into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl AgentLab {
    pub(crate) fn start(label: &str) -> TestValue<Self> {
        let a = Node::start(&format!("{label}-a"))?;
        let b = Node::start(&format!("{label}-b"))?;
        let agents = AgentsStopped([a.state_dir().to_path_buf(), b.state_dir().to_path_buf()]);
        let space = join(&a, &b, "lab")?;
        let root = Root::new(label)?;
        std::fs::write(root.path().join("input.txt"), content(INPUT_LEN))?;
        a.run_ok(&[
            "service",
            "expose",
            "files",
            &space,
            "docs",
            root.as_arg()?,
            "--read-only",
        ])?;
        a.run_ok(&["space", "grant", &space, b.id(), "--files-read"])?;
        a.run_ok(&["space", "grant", &space, a.id(), "--agent"])?;
        b.run_ok(&["service", "expose", "agent", &space, "worker"])?;
        host(&b, "worker", &["transfer", "files"])?;
        host(&a, "lead", &["agent"])?;
        Ok(Self {
            _agents: agents,
            a,
            b,
            space,
            _root: root,
        })
    }

    /// Runs `ma2a agent --endpoint B ARGS…` on `a`.
    pub(crate) fn remote(&self, arguments: &[&str]) -> TestValue<Output> {
        let mut command = vec!["agent", "--endpoint", self.b.id()];
        command.extend(arguments);
        self.a.run(&command)
    }

    /// Spawns `ma2a agent --endpoint B ARGS…` on `a` with stdout piped.
    pub(crate) fn spawn_remote(&self, arguments: &[&str]) -> TestValue<std::process::Child> {
        let mut command = vec!["agent", "--endpoint", self.b.id()];
        command.extend(arguments);
        Ok(self
            .a
            .command(&command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?)
    }

    /// Submits `text` to `worker` from `a` and returns the task ID.
    pub(crate) fn submit_remote(&self, text: &str, request_id: Option<&str>) -> TestValue<String> {
        let mut arguments = vec!["submit", "worker", "--text", text];
        if let Some(request_id) = request_id {
            arguments.extend(["--request-id", request_id]);
        }
        let output = self.remote(&arguments)?;
        super::harness::succeed(&output)?;
        let id = text_of(&last(&output)?, "/task/task_id");
        if id.len() != 32 {
            return Err(format!("submit returned no task: {output:?}").into());
        }
        Ok(id)
    }

    /// Polls `worker`'s view of a task on `b`, as its operator, until it ended.
    pub(crate) fn wait_ended_on_b(&self, task: &str) -> TestValue<Value> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let inspected = self
                .b
                .run(&["agent", "task", task])
                .and_then(|output| last(&output));
            if let Ok(details) = &inspected
                && matches!(
                    text(details, "/task/state").as_str(),
                    "completed" | "failed" | "cancelled"
                )
            {
                return inspected;
            }
            if Instant::now() >= deadline {
                return Err(format!("task never ended: {inspected:?}").into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub(crate) fn output_path(&self, name: &str) -> PathBuf {
        self.a.state_dir().join(name)
    }

    /// Stops both Agents, whose instances otherwise outlive the daemons, then the daemons.
    pub(crate) fn shutdown(self) -> TestResult {
        let _stopped = self.a.run(&["agent", "stop", "lead"]);
        let _stopped = self.b.run(&["agent", "stop", "worker"]);
        let b = self.b.shutdown();
        self.a.shutdown()?;
        b
    }
}

pub(crate) fn text_of(value: &Value, pointer: &str) -> String {
    text(value, pointer)
}
