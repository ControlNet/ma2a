//! Ending every process a Shell session started, including jobs in other process groups.
//!
//! The session program is a session leader on its terminal. A job-control shell moves each
//! `&` job into its own process group, so killing the leader's group is not enough, and the
//! kernel hangs up only the foreground group when the terminal goes away.

use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "android"))]
use rustix::process::kill_process;
use rustix::process::{Pid, Signal, kill_process_group};

/// How long an interactive shell gets to forward a hangup to its jobs.
const HANGUP_GRACE: Duration = Duration::from_millis(100);
/// Upper bound on sweeps, so a fork loop cannot keep cleanup running forever.
#[cfg(any(target_os = "linux", target_os = "android"))]
const SWEEPS: usize = 16;

/// Hangs up the session, then kills its leader's group and every remaining member.
///
/// A program that leaves the session with `setsid` is outside it and is not reached; on
/// hosts without `/proc`, a job that ignores the hangup in another group may also survive.
pub(super) async fn end_session(leader: Pid, leader_running: bool) {
    // Interactive shells resend a hangup to every job before exiting.
    let _hung_up = kill_process_group(leader, Signal::HUP);
    if leader_running {
        tokio::time::sleep(HANGUP_GRACE).await;
    }
    let _killed = kill_process_group(leader, Signal::KILL);
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let _swept = tokio::task::spawn_blocking(move || sweep(leader)).await;
    }
}

/// Kills every live process whose session is the leader's, until none is left.
///
/// Session IDs are inherited and cannot be joined from outside, so a match was started from
/// this session. PID reuse would need a new session leader to take the exact leader PID in
/// the few milliseconds since the leader was reaped.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn sweep(leader: Pid) {
    for _ in 0..SWEEPS {
        let members = members(leader.as_raw_nonzero().get());
        if members.is_empty() {
            return;
        }
        for member in members {
            let _killed = kill_process(member, Signal::KILL);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn members(session: i32) -> Vec<Pid> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&pid| live_session(pid) == Some(session))
        .filter_map(Pid::from_raw)
        .collect()
}

/// Reads the session of a process that has not exited; zombies are already gone.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn live_session(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces or parentheses; fields resume after the last `)`.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let state = fields.next()?;
    let session = fields.nth(2)?.parse().ok()?;
    (state != "Z").then_some(session)
}
