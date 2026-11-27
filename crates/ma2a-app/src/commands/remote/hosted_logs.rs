//! Retained `process logs` reading and post-exit status lookup.

use ma2a_core::service::{
    EndReason, LogFrom, MAX_LOGS_READ, OpenBody, ProcessAccept, ProcessId, ProcessReply,
    ProcessRequest, ServiceError,
};
use ma2a_services::client::{ClientError, answer, hosted};

use super::{
    READ_DEADLINE, bounded,
    hosted::Target,
    hosted_stream::{exit_code, gaps, note},
    open, open_bound,
    stdio::StdioGuard,
};
use crate::AppError;

/// The exit status of a Process that ended before an attachment saw it, as if it had.
pub(super) async fn ended_code(target: Target<'_>, id: ProcessId) -> Result<i32, AppError> {
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

/// Outcome of reading retained output, or detaching early on Ctrl-C.
pub(super) enum Retained {
    /// Finished the retained window at `next`; `ended` reports whether the Process already ended.
    Read { next: u64, ended: bool },
    /// Interrupted before finishing the retained window.
    Detached(Option<u64>),
}

/// Reads the output retained when reading began; returns the cursor and whether the
/// Process ended with everything read, or detaches promptly when `cancel` completes.
///
/// Output written meanwhile is left to the next read, so a Process writing faster than
/// one round trip never keeps this from returning.
pub(super) async fn read_retained(
    (stdio, mut cancel): (&StdioGuard, impl Future<Output = ()> + Unpin),
    target: Target<'_>,
    (id, mut from): (ProcessId, LogFrom),
) -> Result<Retained, AppError> {
    let (mut output, mut error) = (stdio.stdout(), stdio.stderr());
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
        });
        let read = tokio::select! {
            biased;
            () = &mut cancel => return Ok(Retained::Detached(reached)),
            read = read => read?,
        };
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
                note(
                    stdio,
                    &format!("older output was dropped; reading starts at cursor {start}"),
                )
                .await;
            }
        }
        gaps(stdio, read.gaps).await;
        let done = read.ended || until.is_some_and(|until| read.next >= until);
        if !another((reads, reached), (read.next, done))? {
            return Ok(Retained::Read {
                next: read.next,
                ended: read.ended,
            });
        }
        reached = Some(read.next);
        from = LogFrom::Cursor(read.next);
    }
}

/// Most reads of retained output: a Process retains at most 16 MiB, which 16 full reads cover,
/// so a target that keeps the cursor creeping forward further is not followed.
pub(super) const MAX_RETAINED_READS: usize = 64;

/// Whether the `reads`th read, which ended at `next` and is `done` once the Process ended or
/// the read caught up, is followed by another from `next`.
///
/// A cursor that goes back is a faulty reply on every read, the last included, since following
/// or resuming from it would print output already printed. A cursor that does not move would
/// only read the same output again. Only a read that needs another may hit
/// [`MAX_RETAINED_READS`], past which a cursor still moving is a faulty reply too.
pub(super) const fn another(
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
