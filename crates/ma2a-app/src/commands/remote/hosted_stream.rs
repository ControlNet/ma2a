//! `process logs`, `attach`, and `stdin`: output to this terminal, stdin to the Process.

use std::io::Write as _;

use ma2a_core::service::{
    EndReason, LogFrom, MAX_LOGS_READ, OpenBody, ProcessAccept, ProcessId, ProcessRequest,
    ServiceError, Termination,
};
use ma2a_services::client::{
    ClientError,
    hosted::{self, Attached, Gaps},
    process::ProcessIo,
};

use super::{hosted::Target, hosted_view, open, process::interrupted};
use crate::AppError;

/// Prints retained output from `from`; with `follow`, then attaches until the Process ends.
pub(super) async fn logs(
    target: Target<'_>,
    (id, from): (ProcessId, LogFrom),
    follow: bool,
) -> Result<(), AppError> {
    let (next, ended) = read_retained(target, (id, from)).await?;
    if !follow || ended {
        note(&format!("next cursor {next}"));
        return Ok(());
    }
    let (mut reader, mut writer) = open_attach(target, (id, LogFrom::Cursor(next), false)).await?;
    let local = ProcessIo::<tokio::io::Stdin, _, _> {
        input: None,
        output: tokio::io::stdout(),
        error: Some(tokio::io::stderr()),
    };
    match hosted::attach((&mut reader, &mut writer), local, interrupted()).await {
        Ok(attached) => finish(&attached, false),
        // The Process ended between the read and the attach: print what it wrote last.
        Err(ClientError::Rejected(ServiceError::NotRunning)) => {
            let (next, _) = read_retained(target, (id, LogFrom::Cursor(next))).await?;
            note(&format!("next cursor {next}"));
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

/// Reads the output retained when reading began; returns the cursor and whether the
/// Process ended with everything read.
///
/// Output written meanwhile is left to the next read, so a Process writing faster than
/// one round trip never keeps this from returning.
async fn read_retained(
    target: Target<'_>,
    (id, mut from): (ProcessId, LogFrom),
) -> Result<(u64, bool), AppError> {
    let (mut output, mut error) = (tokio::io::stdout(), tokio::io::stderr());
    let mut reached = None;
    let mut until = None;
    loop {
        let request = ProcessRequest::Logs(id, from, MAX_LOGS_READ);
        let (mut reader, writer) = open(target.0, target.1, OpenBody::Process(request)).await?;
        let read = hosted::logs(&mut reader, (&mut output, &mut error)).await?;
        drop(writer);
        if let Some(ProcessAccept::Reading {
            start,
            window,
            truncated,
            ..
        }) = read.accept
        {
            until.get_or_insert(window.end);
            if truncated {
                note(&format!(
                    "older output was dropped; reading starts at cursor {start}"
                ));
            }
        }
        gaps(read.gaps);
        let caught_up = until.is_some_and(|until| read.next >= until);
        if read.ended || caught_up || reached == Some(read.next) {
            return Ok((read.next, read.ended));
        }
        reached = Some(read.next);
        from = LogFrom::Cursor(read.next);
    }
}

/// Follows live output until the Process ends or Ctrl-C detaches.
pub(super) async fn attach(
    target: Target<'_>,
    (id, from, stdin): (ProcessId, LogFrom, bool),
) -> Result<(), AppError> {
    let (mut reader, mut writer) = open_attach(target, (id, from, stdin)).await?;
    let local = ProcessIo {
        input: stdin.then(tokio::io::stdin),
        output: tokio::io::stdout(),
        error: Some(tokio::io::stderr()),
    };
    let attached = hosted::attach((&mut reader, &mut writer), local, interrupted())
        .await
        .map_err(AppError::from);
    finish(&super::unless_reading_stdin(stdin, attached)?, stdin)
}

/// Writes this command's stdin to the Process until EOF.
pub(super) async fn stdin(
    target: Target<'_>,
    (id, close): (ProcessId, bool),
) -> Result<(), AppError> {
    let request = ProcessRequest::Stdin(id, close);
    let (mut reader, mut writer) = open(target.0, target.1, OpenBody::Process(request)).await?;
    let (written, closed) = hosted::stdin((&mut reader, &mut writer), tokio::io::stdin()).await?;
    super::hosted::print(&serde_json::json!({"written": written, "stdin_closed": closed}))
}

async fn open_attach(
    target: Target<'_>,
    (id, from, stdin): (ProcessId, LogFrom, bool),
) -> Result<super::Halves, AppError> {
    let request = ProcessRequest::Attach(id, from, stdin);
    open(target.0, target.1, OpenBody::Process(request)).await
}

/// Reports how the attachment ended and exits: with the Process's own status once it ended,
/// and successfully after a detach.
///
/// The process exits directly because a pending read of stdin cannot be cancelled and
/// would otherwise hold the runtime open.
fn finish(attached: &Attached, reading_stdin: bool) -> Result<(), AppError> {
    gaps(attached.gaps);
    let process = hosted_view::summary(&attached.summary);
    let state = process.get("state").cloned().unwrap_or_default();
    note(&format!(
        "{} at cursor {}; Process {state}",
        if attached.reason == EndReason::Cancelled {
            "detached"
        } else {
            "ended"
        },
        attached.cursor
    ));
    let code = match (attached.reason, attached.summary.termination) {
        (EndReason::Completed, Some(Termination::Exited(code))) => {
            i32::try_from(code.min(255)).unwrap_or(255)
        }
        (EndReason::Completed, Some(Termination::Signaled(signal))) => {
            i32::try_from(128 + signal.min(127)).unwrap_or(255)
        }
        _ => 0,
    };
    std::io::stdout().lock().flush()?;
    std::io::stderr().lock().flush()?;
    if reading_stdin || code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn gaps(gaps: Gaps) {
    if gaps.count > 0 {
        note(&format!(
            "retention dropped older output {} time(s); reading resumed at cursor {}",
            gaps.count,
            gaps.resumed_at.unwrap_or_default()
        ));
    }
}

/// One status line on stderr, apart from the Process's own output on stdout.
fn note(message: &str) {
    let _written = writeln!(std::io::stderr().lock(), "ma2a: {message}");
}
