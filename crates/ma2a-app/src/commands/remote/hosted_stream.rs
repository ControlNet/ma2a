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
    let mut reached = match from {
        LogFrom::Cursor(cursor) => Some(cursor),
        LogFrom::Start | LogFrom::Now => None,
    };
    let (mut until, mut reads) = (None, 0);
    loop {
        reads += 1;
        let request = ProcessRequest::Logs(id, from, MAX_LOGS_READ);
        let (mut reader, writer) = open(target.0, target.1, OpenBody::Process(request)).await?;
        let read = hosted::logs(&mut reader, (&mut output, &mut error), MAX_LOGS_READ).await?;
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
    let written = hosted::stdin((&mut reader, &mut writer), tokio::io::stdin()).await;
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

#[cfg(test)]
mod tests {
    use super::{MAX_RETAINED_READS, another};

    #[test]
    fn a_log_cursor_that_goes_back_is_refused() {
        assert!(another((1, None), (0, false)).is_ok_and(|another| another));
        assert!(another((2, Some(10)), (20, false)).is_ok_and(|another| another));
        assert!(another((2, Some(10)), (10, false)).is_ok_and(|another| !another));
        assert!(another((2, Some(10)), (9, false)).is_err());
    }

    #[test]
    fn a_final_reply_whose_cursor_goes_back_is_refused() {
        // A read from cursor 100 that ends the Process may not resume anyone at 90.
        assert!(another((1, Some(100)), (90, true)).is_err());
        assert!(another((1, Some(100)), (100, true)).is_ok_and(|another| !another));
        assert!(another((1, None), (0, true)).is_ok_and(|another| !another));
    }

    #[test]
    fn a_log_cursor_still_creeping_after_the_last_read_is_refused() {
        let last = MAX_RETAINED_READS;
        assert!(another((last - 1, Some(10)), (11, false)).is_ok_and(|another| another));
        // A last read that ends where the one before it did is answered as it is, and so is
        // one that needs no further read.
        assert!(another((last, Some(10)), (10, false)).is_ok_and(|another| !another));
        assert!(another((last, Some(10)), (11, true)).is_ok_and(|another| !another));
        assert!(another((last, Some(10)), (11, false)).is_err());
    }
}
