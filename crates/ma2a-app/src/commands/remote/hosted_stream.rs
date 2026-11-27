//! `process logs`, `attach`, and `stdin`: output to this terminal, stdin to the Process.

use ma2a_core::service::{
    EndReason, LogFrom, OpenBody, ProcessId, ProcessRequest, ServiceError, Termination,
};
use ma2a_services::client::{
    ClientError,
    hosted::{self, Attached, Gaps},
    process::ProcessIo,
};

#[cfg(test)]
use super::hosted_logs::{MAX_RETAINED_READS, another};
use super::{
    hosted::Target,
    hosted_logs::{ended_code, read_retained},
    hosted_view, open,
    process::interrupted,
    stdio::StdioGuard,
};
use crate::AppError;

/// Prints retained output from `from`; with `follow`, then attaches until the Process ends.
pub(super) async fn logs(
    target: Target<'_>,
    (id, from): (ProcessId, LogFrom),
    follow: bool,
) -> Result<(), AppError> {
    let stdio = StdioGuard::default();
    match logs_exit(&stdio, target, (id, from, follow)).await {
        Ok((code, flush_outputs)) => stdio.exit_code(code, flush_outputs).await,
        Err(error) => stdio.exit_error(error).await,
    }
}

/// Prints as [`logs`] does, and returns the exit status for how it ended.
#[cfg_attr(
    not(all(test, unix)),
    expect(dead_code, reason = "called by Unix test suite")
)]
pub(super) async fn logs_code(
    target: Target<'_>,
    (id, from): (ProcessId, LogFrom),
    follow: bool,
) -> Result<i32, AppError> {
    let stdio = StdioGuard::default();
    Ok(logs_exit(&stdio, target, (id, from, follow)).await?.0)
}

async fn logs_exit(
    stdio: &StdioGuard,
    target: Target<'_>,
    (id, from, follow): (ProcessId, LogFrom, bool),
) -> Result<(i32, bool), AppError> {
    let (next, ended) = read_retained(stdio, target, (id, from)).await?;
    if !follow || ended {
        note(stdio, &format!("next cursor {next}")).await;
        return if ended && follow {
            Ok((ended_code(target, id).await?, true))
        } else {
            Ok((0, true))
        };
    }
    let (mut reader, mut writer) = open_attach(target, (id, LogFrom::Cursor(next), false)).await?;
    let local = ProcessIo::<tokio::io::Stdin, _, _> {
        input: None,
        output: stdio.stdout(),
        error: Some(stdio.stderr()),
    };
    let attach = (id, LogFrom::Cursor(next));
    match hosted::attach((&mut reader, &mut writer), (attach, local), interrupted()).await {
        Ok(attached) => Ok((
            report(stdio, &attached).await,
            attached.reason == EndReason::Completed,
        )),
        // The Process ended between the read and the attach: print what it wrote last.
        Err(ClientError::Rejected(ServiceError::NotRunning)) => {
            let (next, ended) = read_retained(stdio, target, (id, LogFrom::Cursor(next))).await?;
            note(stdio, &format!("next cursor {next}")).await;
            if !ended {
                return Err(ClientError::Rejected(ServiceError::NotRunning).into());
            }
            Ok((ended_code(target, id).await?, true))
        }
        Err(error) => Err(error.into()),
    }
}

/// Follows live output until the Process ends or Ctrl-C detaches.
pub(super) async fn attach(
    target: Target<'_>,
    (id, from, stdin): (ProcessId, LogFrom, bool),
) -> Result<(), AppError> {
    let guard = StdioGuard::default();
    let (mut reader, mut writer) = match open_attach(target, (id, from, stdin)).await {
        Ok(halves) => halves,
        Err(error) => guard.exit_error(error).await,
    };
    let local = ProcessIo {
        input: stdin.then(tokio::io::stdin),
        output: guard.stdout(),
        error: Some(guard.stderr()),
    };
    match hosted::attach(
        (&mut reader, &mut writer),
        ((id, from), local),
        interrupted(),
    )
    .await
    {
        Ok(attached) => finish(&guard, &attached).await,
        Err(error) => guard.exit_error(error.into()).await,
    }
}

/// Writes this command's stdin to the Process until EOF.
pub(super) async fn stdin(
    target: Target<'_>,
    (id, close): (ProcessId, bool),
) -> Result<(), AppError> {
    let request = ProcessRequest::Stdin(id, close);
    let (mut reader, mut writer) = open(target.0, target.1, OpenBody::Process(request)).await?;
    let written = hosted::stdin((&mut reader, &mut writer), (tokio::io::stdin(), close)).await;
    // The Process may end while stdin is still being read, so every outcome exits here.
    super::exit_with(
        written
            .map_err(AppError::from)
            .and_then(|(written, closed)| {
                super::hosted::print(
                    &serde_json::json!({"written": written, "stdin_closed": closed}),
                )
            }),
    )
}

async fn open_attach(
    target: Target<'_>,
    (id, from, stdin): (ProcessId, LogFrom, bool),
) -> Result<super::Halves, AppError> {
    let request = ProcessRequest::Attach(id, from, stdin);
    open(target.0, target.1, OpenBody::Process(request)).await
}

/// Reports how the attachment ended and exits directly without waiting on stuck stdio or stdin
/// workers: with the Process's own status once it ended, and 0 after a detach.
async fn finish(stdio: &StdioGuard, attached: &Attached) -> ! {
    let code = report(stdio, attached).await;
    stdio
        .exit_code(code, attached.reason == EndReason::Completed)
        .await
}

/// Reports how the attachment ended, and returns the exit status for it.
async fn report(stdio: &StdioGuard, attached: &Attached) -> i32 {
    gaps(stdio, attached.gaps).await;
    let process = hosted_view::summary(&attached.summary);
    let state = process.get("state").cloned().unwrap_or_default();
    note(
        stdio,
        &format!(
            "{} at cursor {}; Process {state}",
            if attached.reason == EndReason::Cancelled {
                "detached"
            } else {
                "ended"
            },
            attached.cursor
        ),
    )
    .await;
    exit_code(attached.reason, attached.summary.termination)
}

/// The exit status for how an attachment ended: the Process's own once it ended, as for
/// `exec`, 255 when it ended without one, and success after a detach.
pub(super) fn exit_code(reason: EndReason, termination: Option<Termination>) -> i32 {
    match (reason, termination) {
        (EndReason::Completed, Some(Termination::Exited(code))) => {
            i32::try_from(code.min(255)).unwrap_or(255)
        }
        (EndReason::Completed, Some(Termination::Signaled(signal))) => {
            i32::try_from(128 + signal.min(127)).unwrap_or(255)
        }
        (EndReason::Completed, _) => 255,
        _ => 0,
    }
}

pub(super) async fn gaps(stdio: &StdioGuard, gaps: Gaps) {
    if gaps.count > 0 {
        note(
            stdio,
            &format!(
                "retention dropped older output {} time(s); reading resumed at cursor {}",
                gaps.count,
                gaps.resumed_at.unwrap_or_default()
            ),
        )
        .await;
    }
}

/// One status line on stderr, apart from the Process's own output on stdout.
pub(super) async fn note(stdio: &StdioGuard, message: &str) {
    stdio.note_stderr(&format!("ma2a: {message}")).await;
}

#[cfg(test)]
#[path = "hosted_stream_tests.rs"]
mod tests;
