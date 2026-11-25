//! `process logs`, `attach`, and `stdin`: output to this terminal, stdin to the Process.

use std::io::Write as _;

use ma2a_core::service::{
    EndReason, LogFrom, MAX_LOGS_READ, OpenBody, ProcessAccept, ProcessId, ProcessReply,
    ProcessRequest, ServiceError, Termination,
};
use ma2a_services::client::{
    ClientError, answer,
    hosted::{self, Attached, Gaps},
    process::ProcessIo,
};

use super::{
    READ_DEADLINE, bounded, hosted::Target, hosted_view, open, open_bound, process::interrupted,
};
use crate::AppError;

/// Prints retained output from `from`; with `follow`, then attaches until the Process ends.
pub(super) async fn logs(
    target: Target<'_>,
    read: (ProcessId, LogFrom),
    follow: bool,
) -> Result<(), AppError> {
    let code = logs_code(target, read, follow).await?;
    exit(code, false)
}

/// Prints as [`logs`] does, and returns the exit status for how it ended.
pub(super) async fn logs_code(
    target: Target<'_>,
    (id, from): (ProcessId, LogFrom),
    follow: bool,
) -> Result<i32, AppError> {
    let (next, ended) = read_retained(target, (id, from)).await?;
    if !follow || ended {
        note(&format!("next cursor {next}"));
        return if ended && follow {
            ended_code(target, id).await
        } else {
            Ok(0)
        };
    }
    let (mut reader, mut writer) = open_attach(target, (id, LogFrom::Cursor(next), false)).await?;
    let local = ProcessIo::<tokio::io::Stdin, _, _> {
        input: None,
        output: tokio::io::stdout(),
        error: Some(tokio::io::stderr()),
    };
    let attach = (id, LogFrom::Cursor(next));
    match hosted::attach((&mut reader, &mut writer), (attach, local), interrupted()).await {
        Ok(attached) => Ok(report(&attached)),
        // The Process ended between the read and the attach: print what it wrote last.
        Err(ClientError::Rejected(ServiceError::NotRunning)) => {
            let (next, ended) = read_retained(target, (id, LogFrom::Cursor(next))).await?;
            note(&format!("next cursor {next}"));
            if !ended {
                return Err(ClientError::Rejected(ServiceError::NotRunning).into());
            }
            ended_code(target, id).await
        }
        Err(error) => Err(error.into()),
    }
}

/// The exit status of a Process that ended before an attachment saw it, as if it had.
async fn ended_code(target: Target<'_>, id: ProcessId) -> Result<i32, AppError> {
    let request = ProcessRequest::Inspect(id);
    let body = OpenBody::Process(request.clone());
    let (opened, (accept, reply)) = bounded(READ_DEADLINE, async {
        let (opened, (mut reader, writer)) = open_bound(target.0, (target.1, None), body).await?;
        let answer = hosted::call(&mut reader).await?;
        drop(writer);
        Ok((opened, answer))
    })
    .await?;
    match &reply {
        ProcessReply::Details(details)
            if answer::process((&request, &opened), (accept, &reply)) =>
        {
            Ok(exit_code(EndReason::Completed, details.summary.termination))
        }
        _ => Err(AppError::Protocol(
            "the target answered another Process or operation".to_owned(),
        )),
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
    let mut reached = match from {
        LogFrom::Cursor(cursor) => Some(cursor),
        LogFrom::Start | LogFrom::Now => None,
    };
    let (mut until, mut reads) = (None, 0);
    loop {
        reads += 1;
        let request = ProcessRequest::Logs(id, from, MAX_LOGS_READ);
        let read = bounded(READ_DEADLINE, async {
            let (mut reader, writer) = open(target.0, target.1, OpenBody::Process(request)).await?;
            let read = hosted::logs(
                &mut reader,
                (&mut output, &mut error),
                (from, MAX_LOGS_READ),
            )
            .await;
            drop(writer);
            Ok(read?)
        })
        .await?;
        // The read was checked to start where `from` resolves, so never before `reached`.
        if let ProcessAccept::Reading {
            start,
            window,
            truncated,
            ..
        } = read.accept
        {
            reached = Some(start);
            until.get_or_insert(window.end);
            if truncated {
                note(&format!(
                    "older output was dropped; reading starts at cursor {start}"
                ));
            }
        }
        gaps(read.gaps);
        let done = read.ended || until.is_some_and(|until| read.next >= until);
        if !another((reads, reached), (read.next, done))? {
            return Ok((read.next, read.ended));
        }
        reached = Some(read.next);
        from = LogFrom::Cursor(read.next);
    }
}

/// Most reads of retained output: a Process retains at most 16 MiB, which 16 full reads cover,
/// so a target that keeps the cursor creeping forward further is not followed.
const MAX_RETAINED_READS: usize = 64;

/// Whether the `reads`th read, which ended at `next` and is `done` once the Process ended or
/// the read caught up, is followed by another from `next`.
///
/// A cursor that goes back is a faulty reply on every read, the last included, since following
/// or resuming from it would print output already printed. A cursor that does not move would
/// only read the same output again. Only a read that needs another may hit
/// [`MAX_RETAINED_READS`], past which a cursor still moving is a faulty reply too.
const fn another(
    (reads, reached): (usize, Option<u64>),
    (next, done): (u64, bool),
) -> Result<bool, ClientError> {
    match reached {
        Some(reached) if next < reached => Err(ClientError::Stream(ServiceError::InvalidInput)),
        Some(reached) if next == reached => Ok(false),
        _ if done => Ok(false),
        _ if reads >= MAX_RETAINED_READS => Err(ClientError::Stream(ServiceError::InvalidInput)),
        _ => Ok(true),
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
    let attached = hosted::attach(
        (&mut reader, &mut writer),
        ((id, from), local),
        interrupted(),
    )
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

/// Reports how the attachment ended and exits: with the Process's own status once it ended,
/// and successfully after a detach.
fn finish(attached: &Attached, reading_stdin: bool) -> Result<(), AppError> {
    exit(report(attached), reading_stdin)
}

/// Reports how the attachment ended, and returns the exit status for it.
fn report(attached: &Attached) -> i32 {
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
    exit_code(attached.reason, attached.summary.termination)
}

/// Exits with `code` once output is flushed; returns only on success while stdin is not read.
///
/// The process exits directly because a pending read of stdin cannot be cancelled and
/// would otherwise hold the runtime open.
fn exit(code: i32, reading_stdin: bool) -> Result<(), AppError> {
    std::io::stdout().lock().flush()?;
    std::io::stderr().lock().flush()?;
    if reading_stdin || code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// The exit status for how an attachment ended: the Process's own once it ended, as for
/// `exec`, 255 when it ended without one, and success after a detach.
fn exit_code(reason: EndReason, termination: Option<Termination>) -> i32 {
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

#[cfg(test)]
#[path = "hosted_stream_tests.rs"]
mod tests;
