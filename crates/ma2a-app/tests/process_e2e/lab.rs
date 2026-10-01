//! Two joined daemons: `owner` hosts Processes for Space `lab`, where `member` holds the
//! `process` grant.
//!
//! Every workload here is bounded, so a Process a failed test leaves behind still ends by
//! itself within two minutes.

use std::{
    io::{BufRead as _, BufReader},
    process::{Child, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{
    daemon_fixture::{TestResult, TestValue},
    harness::{Node, join, text},
};

/// Prints `tick N` ten times a second for at most 30 s.
pub(crate) const TICKER: &str =
    "i=0; while [ $i -lt 300 ]; do echo tick $i; i=$((i+1)); sleep 0.1; done";

/// Prints `tick N` ten times a second for at most 120 s.
///
/// For tests that crash the owner: a caller notices a peer that died without closing its
/// connections only at the QUIC idle timeout (30 s), and the workload must outlive that.
pub(crate) const LONG_TICKER: &str =
    "i=0; while [ $i -lt 1200 ]; do echo tick $i; i=$((i+1)); sleep 0.1; done";

pub(crate) struct ProcessLab {
    pub(crate) owner: Node,
    pub(crate) member: Node,
    pub(crate) space: String,
}

impl ProcessLab {
    pub(crate) fn start(label: &str) -> TestValue<Self> {
        let owner = Node::start(&format!("{label}-owner"))?;
        let member = Node::start(&format!("{label}-member"))?;
        let space = join(&owner, &member, "lab")?;
        owner.run_ok(&["service", "expose", "process", &space])?;
        owner.run_ok(&["space", "grant", &space, member.id(), "--process"])?;
        Ok(Self {
            owner,
            member,
            space,
        })
    }

    /// Runs `ma2a process --endpoint OWNER ARGS…` on the member.
    pub(crate) fn remote(&self, arguments: &[&str]) -> TestValue<Output> {
        self.remote_from(&self.member, arguments)
    }

    /// Runs `ma2a process --endpoint OWNER ARGS…` on `caller`.
    pub(crate) fn remote_from(&self, caller: &Node, arguments: &[&str]) -> TestValue<Output> {
        let mut command = vec!["process", "--endpoint", self.owner.id()];
        command.extend(arguments);
        caller.run(&command)
    }

    /// Runs one remote command that must succeed and parses its JSON answer.
    pub(crate) fn json(&self, arguments: &[&str]) -> TestValue<Value> {
        let output = self.remote(arguments)?;
        if !output.status.success() {
            return Err(format!(
                "process {arguments:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }

    /// Starts one Process from the member and returns its ID.
    pub(crate) fn start_process(&self, arguments: &[&str]) -> TestValue<String> {
        let mut command = vec!["start"];
        command.extend(arguments);
        let started = self.json(&command)?;
        let id = text(&started, "/process_id");
        if id.len() != 32 {
            return Err(format!("start returned no Process ID: {started}").into());
        }
        Ok(id)
    }

    /// Polls the member's view until the Process is in `state`.
    pub(crate) fn wait_state(&self, id: &str, state: &str) -> TestValue<Value> {
        Self::wait_until((id, state), |id| self.json(&["inspect", id]))
    }

    /// Polls the host operator's own view until the Process is in `state`.
    pub(crate) fn wait_local_state(&self, id: &str, state: &str) -> TestValue<Value> {
        Self::wait_until((id, state), |id| {
            let output = self.owner.run_ok(&["process", "inspect", id])?;
            Ok(serde_json::from_slice(&output.stdout)?)
        })
    }

    fn wait_until(
        (id, state): (&str, &str),
        inspect: impl Fn(&str) -> TestValue<Value>,
    ) -> TestValue<Value> {
        // Longer than the QUIC idle timeout after which a caller abandons a crashed peer.
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            // A daemon that just restarted may fail a first request on a stale connection.
            let inspected = match inspect(id) {
                Ok(inspected) if text(&inspected, "/process/state") == state => {
                    return Ok(inspected);
                }
                Ok(inspected) => inspected.to_string(),
                Err(error) => error.to_string(),
            };
            if Instant::now() >= deadline {
                return Err(format!("Process never reached {state}: {inspected}").into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Spawns a remote command on the member with stdout and stderr piped.
    pub(crate) fn spawn(&self, arguments: &[&str]) -> TestValue<Child> {
        let mut command = vec!["process", "--endpoint", self.owner.id()];
        command.extend(arguments);
        Ok(self.member.command(&command).stdin(Stdio::null()).spawn()?)
    }

    /// Terminates every Process the owner still hosts, then stops both daemons.
    pub(crate) fn shutdown(self) -> TestResult {
        let listed = self.owner.run_ok(&["process", "list"])?;
        let listed: Value = serde_json::from_slice(&listed.stdout)?;
        let processes = listed
            .pointer("/processes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for process in processes {
            if matches!(text(&process, "/state").as_str(), "starting" | "running") {
                let id = text(&process, "/process_id");
                self.owner
                    .run_ok(&["process", "terminate", &id, "--grace", "500ms"])?;
            }
        }
        let member = self.member.shutdown();
        self.owner.shutdown()?;
        member
    }
}

/// Reads the first stdout line of `child` within 20 s.
pub(crate) fn first_line(child: &mut Child) -> TestValue<String> {
    let stdout = child.stdout.take().ok_or("missing stdout")?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let read = reader.read_line(&mut line).map(|_| line);
        let _sent = sender.send(read);
        // Keep draining so the attachment never blocks on a full pipe.
        let _drained = std::io::copy(&mut reader, &mut std::io::sink());
    });
    Ok(receiver.recv_timeout(Duration::from_secs(20))??)
}

/// Sends `signal` to one local process.
pub(crate) fn signal(child: &Child, signal: rustix::process::Signal) -> TestResult {
    let pid = rustix::process::Pid::from_raw(i32::try_from(child.id())?).ok_or("no pid")?;
    rustix::process::kill_process(pid, signal)?;
    Ok(())
}
