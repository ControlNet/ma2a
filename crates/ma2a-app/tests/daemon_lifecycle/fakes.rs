//! Stand-ins for daemons this executable cannot build: older, newer, or stuck.
//!
//! A real daemon of another version, or one that predates the lifecycle plane,
//! is not something a test can produce, so these speak exactly the parts of the
//! wire contract such a daemon would and nothing more. Each one records what it
//! was asked, which is how a test proves which plane a command actually used.

use std::{
    fs::{self, OpenOptions},
    io::{self, Read as _, Write as _},
    os::unix::{
        fs::PermissionsExt as _,
        net::{UnixListener, UnixStream},
    },
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

use serde_json::{Value, json};

use super::{DaemonFixture, TestValue};

/// Which plane the stand-in daemon speaks.
#[derive(Clone, Copy)]
pub(super) enum Plane {
    /// Answers `ma2a_lifecycle` calls, as every daemon since that plane does.
    Lifecycle,
    /// Predates the lifecycle plane and answers it with a local API error.
    Legacy,
}

/// What the stand-in daemon does when asked to shut down.
#[derive(Clone, Copy)]
pub(super) enum Shutdown {
    /// Acknowledges, then exits and releases the daemon lock.
    Acknowledge,
    /// Accepts the request and never replies, like a daemon whose Runtime is stuck.
    Ignore,
}

#[derive(Clone, Copy)]
pub(super) struct Fake {
    pub(super) plane: Plane,
    pub(super) api_version: u64,
    pub(super) shutdown: Shutdown,
    /// Whether it holds the daemon lock, as a real daemon always does.
    pub(super) owns_lock: bool,
}

pub(super) struct FakeDaemon {
    stopped: Arc<AtomicBool>,
    operations: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl FakeDaemon {
    /// Binds the fixture's endpoint and serves it until told to stop or asked to exit.
    pub(super) fn serve(fixture: &DaemonFixture, fake: Fake) -> TestValue<Self> {
        let runtime_dir = fixture.runtime_dir();
        fs::create_dir_all(&runtime_dir)?;
        fs::set_permissions(&runtime_dir, fs::Permissions::from_mode(0o700))?;
        let lock = if fake.owns_lock {
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(runtime_dir.join("daemon.lock"))?;
            lock.lock()?;
            Some(lock)
        } else {
            None
        };
        let endpoint = runtime_dir.join("control.sock");
        let listener = UnixListener::bind(&endpoint)?;
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let stopped = Arc::new(AtomicBool::new(false));
        let operations = Arc::new(Mutex::new(Vec::new()));
        let thread = std::thread::spawn({
            let stopped = Arc::clone(&stopped);
            let operations = Arc::clone(&operations);
            move || {
                // Held for exactly as long as this stand-in serves, like a daemon's.
                let _lock = lock;
                #[expect(
                    clippy::collection_is_never_read,
                    reason = "held open so a caller waiting on a reply never sees end-of-file"
                )]
                let mut unanswered = Vec::new();
                while !stopped.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _address)) => {
                            // A caller that gives up mid-exchange breaks one
                            // connection, not the daemon it was talking to.
                            match answer(stream, fake, &operations) {
                                Ok(Next::Continue) | Err(_) => {}
                                Ok(Next::Hold(stream)) => unanswered.push(stream),
                                Ok(Next::Exit) => return Ok(()),
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            }
        });
        Ok(Self {
            stopped,
            operations,
            thread: Some(thread),
        })
    }

    /// Every request this stand-in was sent, in order.
    pub(super) fn operations(&self) -> Vec<String> {
        self.operations
            .lock()
            .map(|operations| operations.clone())
            .unwrap_or_default()
    }

    /// Reports whether it has exited on its own, as a real daemon does after stopping.
    pub(super) fn has_exited(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Stops serving, releases the lock, and returns what it was asked.
    pub(super) fn finish(mut self) -> TestValue<Vec<String>> {
        self.stop()?;
        Ok(self.operations())
    }

    fn stop(&mut self) -> TestValue<()> {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| "the fake daemon panicked")??;
        }
        Ok(())
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        let _stopped = self.stop();
    }
}

enum Next {
    Continue,
    Hold(UnixStream),
    Exit,
}

fn answer(mut stream: UnixStream, fake: Fake, operations: &Mutex<Vec<String>>) -> io::Result<Next> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut header = [0_u8; 12];
    stream.read_exact(&mut header)?;
    let length = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let mut payload = vec![0_u8; usize::try_from(length).unwrap_or(0)];
    stream.read_exact(&mut payload)?;
    let request: Value = serde_json::from_slice(&payload)?;
    let version = fake.api_version;
    let (operation, reply, next) = if request.get("ma2a_lifecycle").is_some() {
        let operation = request
            .get("op")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match fake.plane {
            Plane::Lifecycle if operation == "stop" => {
                ("lifecycle:stop", report(version, "stopping"), Next::Exit)
            }
            Plane::Lifecycle => ("lifecycle:ping", report(version, "pong"), Next::Continue),
            Plane::Legacy => (
                "lifecycle:unsupported",
                json!({"version": version, "request_id": null, "error": "invalid_request"}),
                Next::Continue,
            ),
        }
    } else {
        match request.get("operation").and_then(Value::as_str) {
            Some("handshake") => (
                "handshake",
                json!({"version": version, "request_id": null, "revision": 0, "result": {"type": "handshake"}}),
                Next::Continue,
            ),
            Some("graceful_shutdown") => {
                if matches!(fake.shutdown, Shutdown::Ignore) {
                    record(operations, "graceful_shutdown");
                    return Ok(Next::Hold(stream));
                }
                (
                    "graceful_shutdown",
                    json!({
                        "version": version,
                        "request_id": request.get("request_id"),
                        "revision": 0,
                        "result": {"type": "shutting_down"},
                    }),
                    Next::Exit,
                )
            }
            _ => (
                "other",
                json!({"version": version, "request_id": null, "error": "invalid_request"}),
                Next::Continue,
            ),
        }
    };
    record(operations, operation);
    let reply = serde_json::to_vec(&reply)?;
    stream.write_all(&u32::try_from(reply.len()).unwrap_or(0).to_be_bytes())?;
    stream.write_all(&header[4..])?;
    stream.write_all(&reply)?;
    Ok(next)
}

fn record(operations: &Mutex<Vec<String>>, operation: &str) {
    if let Ok(mut operations) = operations.lock() {
        operations.push(operation.to_owned());
    }
}

/// The lifecycle record a daemon of `api_version` would send about itself.
fn report(api_version: u64, operation: &str) -> Value {
    json!({
        "ma2a_lifecycle": 1,
        "op": operation,
        "api_version": api_version,
        "binary_version": "99.0.0",
        "pid": std::process::id(),
        "process_started_at": null,
        "launch_nonce": null,
        "runtime_boot_id": "ab".repeat(16),
        "endpoint_id": "cd".repeat(32),
    })
}
