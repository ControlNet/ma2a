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
    hosted_logs::{Interrupt, Retained, ended_code, read_retained},
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

async fn detach_note(stdio: &StdioGuard, cursor: Option<u64>) -> (i32, bool) {
    match cursor {
        Some(cursor) => note(stdio, &format!("detached at cursor {cursor}")).await,
        None => note(stdio, "detached").await,
    }
    (0, false)
}

async fn ended_or_detach(
    (stdio, interrupt): (&StdioGuard, &mut Interrupt),
    target: Target<'_>,
    (id, cursor): (ProcessId, u64),
) -> Result<(i32, bool), AppError> {
    tokio::select! {
        biased;
        () = interrupt => Ok(detach_note(stdio, Some(cursor)).await),
        code = ended_code(target, id) => Ok((code?, true)),
    }
}

async fn logs_exit(
    stdio: &StdioGuard,
    target: Target<'_>,
    (id, from, follow): (ProcessId, LogFrom, bool),
) -> Result<(i32, bool), AppError> {
    let mut interrupt = Interrupt::new(follow);
    let (next, ended) = match read_retained((stdio, &mut interrupt), target, (id, from)).await? {
        Retained::Read { next, ended } => (next, ended),
        Retained::Detached(cursor) => return Ok(detach_note(stdio, cursor).await),
    };
    if !follow || ended {
        note(stdio, &format!("next cursor {next}")).await;
        return if ended && follow {
            ended_or_detach((stdio, &mut interrupt), target, (id, next)).await
        } else {
            Ok((0, true))
        };
    }
    let (mut reader, mut writer) = tokio::select! {
        biased;
        () = &mut interrupt => return Ok(detach_note(stdio, Some(next)).await),
        opened = open_attach(target, (id, LogFrom::Cursor(next), false)) => opened?,
    };
    let local = ProcessIo::<tokio::io::Stdin, _, _> {
        input: None,
        output: stdio.stdout(),
        error: Some(stdio.stderr()),
    };
    let attach = (id, LogFrom::Cursor(next));
    match hosted::attach((&mut reader, &mut writer), (attach, local), &mut interrupt).await {
        Ok(attached) => Ok(report(stdio, &attached).await),
        // The Process ended between the read and the attach: print what it wrote last.
        Err(ClientError::Rejected(ServiceError::NotRunning)) => {
            if interrupt.is_fired() {
                return Ok(detach_note(stdio, Some(next)).await);
            }
            let (next, ended) =
                match read_retained((stdio, &mut interrupt), target, (id, LogFrom::Cursor(next)))
                    .await?
                {
                    Retained::Read { next, ended } => (next, ended),
                    Retained::Detached(cursor) => return Ok(detach_note(stdio, cursor).await),
                };
            note(stdio, &format!("next cursor {next}")).await;
            if !ended {
                return Err(ClientError::Rejected(ServiceError::NotRunning).into());
            }
            ended_or_detach((stdio, &mut interrupt), target, (id, next)).await
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
    let (code, flush) = report(stdio, attached).await;
    stdio.exit_code(code, flush).await
}

/// Effective reason for reporting and exit: interrupted local output delivery is a detach.
const fn effective_reason(reason: EndReason, delivery_interrupted: bool) -> EndReason {
    if delivery_interrupted {
        EndReason::Cancelled
    } else {
        reason
    }
}

/// Reports how the attachment ended, returning the exit status and whether outputs should flush.
async fn report(stdio: &StdioGuard, attached: &Attached) -> (i32, bool) {
    gaps(stdio, attached.gaps).await;
    let reason = effective_reason(attached.reason, attached.delivery_interrupted);
    let process = hosted_view::summary(&attached.summary);
    let state = process.get("state").cloned().unwrap_or_default();
    note(
        stdio,
        &format!(
            "{} at cursor {}; Process {state}",
            if reason == EndReason::Cancelled {
                "detached"
            } else {
                "ended"
            },
            attached.cursor
        ),
    )
    .await;
    (
        exit_code(reason, attached.summary.termination),
        reason == EndReason::Completed,
    )
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
