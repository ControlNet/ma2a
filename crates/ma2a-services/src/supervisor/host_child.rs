//! The lifecycle of one supervised child: two-phase start, output, signals, and its end.

use std::{process::Stdio, sync::Arc, time::Duration};

use ma2a_core::service::{Channel, ProcessSignal, ServiceError, Termination};
use tokio::{
    io::AsyncReadExt as _,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

use super::{
    host::{Lifecycle, Shared},
    status::{EndCause, SupervisorStatus},
};
use crate::process::{self, Spawned};

/// Without `go` in this time the supervisor exits and no child ever exists.
pub const GO_DEADLINE: Duration = Duration::from_secs(30);
/// Bound on draining output left in the pipes after the child ended.
const DRAIN: Duration = Duration::from_secs(5);
const CHUNK: usize = 32_768;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Runs the lifecycle to its final status.
pub(super) async fn lifecycle(shared: Arc<Shared>, mut requests: mpsc::Receiver<Lifecycle>) {
    let Some(go) = await_go(&shared, &mut requests).await else {
        return;
    };
    let running = spawn(&shared).await;
    let _sent = go.send(*shared.status.borrow());
    if let Some(running) = running {
        run(&shared, running, &mut requests).await;
    }
}

/// Waits for `go`; every other request is answered as if no child exists yet.
async fn await_go(
    shared: &Shared,
    requests: &mut mpsc::Receiver<Lifecycle>,
) -> Option<oneshot::Sender<SupervisorStatus>> {
    let deadline = tokio::time::sleep(GO_DEADLINE);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            () = &mut deadline => break,
            request = requests.recv() => match request {
                // The reply follows the spawn, once the status is known.
                Some(Lifecycle::Go(reply)) => return Some(reply),
                Some(Lifecycle::Abort(reply)) => {
                    shared.record(SupervisorStatus::NeverStarted(now_ms())).await;
                    let _sent = reply.send(Ok(*shared.status.borrow()));
                    return None;
                }
                Some(Lifecycle::Signal(_, reply)) => {
                    let _sent = reply.send(Err(ServiceError::NotRunning));
                }
                Some(Lifecycle::Terminate(_, reply)) => {
                    shared.record(SupervisorStatus::NeverStarted(now_ms())).await;
                    let _sent = reply.send(*shared.status.borrow());
                    return None;
                }
                None => break,
            },
        }
    }
    shared
        .record(SupervisorStatus::NeverStarted(now_ms()))
        .await;
    None
}

type Pumps = [JoinHandle<()>; 2];

async fn spawn(shared: &Arc<Shared>) -> Option<(Spawned, Pumps)> {
    let spec = shared.start.spec();
    let spawned = spec
        .program()
        .ok_or(ServiceError::SpawnFailed)
        .and_then(|program| process::command(program, spec))
        .and_then(|mut command| {
            command
                .stdin(if shared.start.stdin() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            command.spawn().map_err(|_| ServiceError::SpawnFailed)
        });
    let Ok(mut child) = spawned else {
        shared.record(SupervisorStatus::SpawnFailed(now_ms())).await;
        return None;
    };
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    if let Some(stdin) = child.stdin.take()
        && let Ok(mut slot) = shared.stdin.try_lock()
    {
        *slot = Some(stdin);
    }
    let pid = child.id().unwrap_or(0);
    shared
        .record(SupervisorStatus::Running(pid, now_ms()))
        .await;
    let pumps = [
        tokio::spawn(pump(Arc::clone(shared), Channel::Output, stdout)),
        tokio::spawn(pump(Arc::clone(shared), Channel::Error, stderr)),
    ];
    Some((Spawned::new(child), pumps))
}

async fn pump<R: tokio::io::AsyncRead + Unpin>(
    shared: Arc<Shared>,
    channel: Channel,
    source: Option<R>,
) {
    let Some(mut source) = source else {
        return;
    };
    let mut buffer = vec![0; CHUNK];
    while let Ok(read) = source.read(&mut buffer).await {
        let Some(bytes) = buffer.get(..read).filter(|bytes| !bytes.is_empty()) else {
            return;
        };
        shared.append(channel, bytes);
    }
}

/// Serves requests while the child runs, then records how it ended.
async fn run(
    shared: &Arc<Shared>,
    (mut spawned, pumps): (Spawned, Pumps),
    requests: &mut mpsc::Receiver<Lifecycle>,
) {
    let mut terminating: Option<(tokio::time::Instant, Vec<oneshot::Sender<SupervisorStatus>>)> =
        None;
    // The grace period ends in one kill; a child that outlives it is waited for, not killed
    // again on every turn of this loop.
    let mut killed = false;
    let status = loop {
        let kill_at = terminating.as_ref().map(|(at, _)| *at).filter(|_| !killed);
        tokio::select! {
            status = spawned.child.wait() => break status.ok(),
            () = sleep_until(kill_at) => {
                spawned.kill_all();
                killed = true;
            }
            request = requests.recv() => match request {
                Some(Lifecycle::Go(reply)) => {
                    let _sent = reply.send(*shared.status.borrow());
                }
                Some(Lifecycle::Abort(reply)) => {
                    let _sent = reply.send(Err(ServiceError::ProcessActive));
                }
                Some(Lifecycle::Signal(signal, reply)) => {
                    let _sent = reply.send(signal_group(&mut spawned, signal));
                }
                Some(Lifecycle::Terminate(grace_ms, reply)) => {
                    let requested = tokio::time::Instant::now()
                        + Duration::from_millis(u64::from(grace_ms));
                    let (deadline, waiters) = terminating.get_or_insert_with(|| {
                        let _signalled = signal_group(&mut spawned, ProcessSignal::Terminate);
                        (requested, Vec::new())
                    });
                    // A later, stricter termination (for example an operator kill or revoked
                    // authorization) can always shorten an existing grace deadline.
                    if requested < *deadline {
                        *deadline = requested;
                    }
                    waiters.push(reply);
                }
                None => {}
            },
        }
    };
    // Anything the child left in its process group ends with it.
    spawned.kill_all();
    for pump in pumps {
        let _drained = tokio::time::timeout(DRAIN, pump).await;
    }
    shared.sync_log().await;
    #[cfg(debug_assertions)]
    if std::env::var_os("MA2A_TEST_SUPERVISOR_FAULT").is_some_and(|point| point == "before_status")
    {
        super::crash();
    }
    let termination = status.map_or(Termination::Unknown, process::termination);
    let cause = if terminating.is_some() {
        EndCause::Terminated
    } else {
        EndCause::Exited
    };
    shared
        .record(SupervisorStatus::Ended(termination, cause, now_ms()))
        .await;
    for waiter in terminating.map(|(_, waiters)| waiters).unwrap_or_default() {
        let _sent = waiter.send(*shared.status.borrow());
    }
}

async fn sleep_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Sends one signal to the child's process group; Windows can only end the child.
#[cfg_attr(
    unix,
    expect(
        clippy::needless_pass_by_ref_mut,
        reason = "Windows ends the child through it"
    )
)]
fn signal_group(spawned: &mut Spawned, signal: ProcessSignal) -> Result<(), ServiceError> {
    #[cfg(unix)]
    {
        use rustix::process::{Pid, Signal, kill_process_group};
        let raw = match signal {
            ProcessSignal::Interrupt => Signal::INT,
            ProcessSignal::Terminate => Signal::TERM,
            ProcessSignal::Kill => Signal::KILL,
            ProcessSignal::Hangup => Signal::HUP,
            ProcessSignal::Quit => Signal::QUIT,
            ProcessSignal::User1 => Signal::USR1,
            ProcessSignal::User2 => Signal::USR2,
        };
        let group = spawned
            .group()
            .and_then(|id| i32::try_from(id).ok())
            .and_then(Pid::from_raw)
            .ok_or(ServiceError::NotRunning)?;
        kill_process_group(group, raw).map_err(|_| ServiceError::NotRunning)
    }
    #[cfg(windows)]
    match signal {
        ProcessSignal::Terminate | ProcessSignal::Kill => {
            spawned.kill_all();
            Ok(())
        }
        _ => Err(ServiceError::InvalidInput),
    }
}
