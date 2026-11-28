//! `ma2a process`: durable Processes on another Endpoint, or hosted by this one.

use std::{io::Write as _, path::Path};

use ma2a_core::{
    RequestId,
    service::{
        DEFAULT_LOG_LIMIT, LogFrom, MAX_TERMINATE_GRACE_MS, OpenBody, ProcessAccept, ProcessId,
        ProcessReply, ProcessRequest, ProcessSignal, ProcessSpec, ProcessStart,
    },
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client::{answer, hosted};
use serde_json::{Value, json};

use super::{READ_DEADLINE, bounded, duration_within, hosted_stream, hosted_view, open_bound};
use crate::{
    AppError,
    cli::processes::{ProcessArgs, ProcessCommand, StartArgs},
    commands::workflows::{command, request_fields},
};

/// One remote target: the local stream plane and the hosting Endpoint.
pub(super) type Target<'a> = (&'a IpcPaths, &'a str);

pub(crate) async fn run(
    state_dir: &Path,
    paths: IpcPaths,
    arguments: ProcessArgs,
) -> Result<(), AppError> {
    let Some(endpoint) = arguments.endpoint else {
        return local(state_dir, paths, arguments.command).await;
    };
    let target = (&paths, endpoint.as_str());
    let request = match arguments.command {
        ProcessCommand::Start(start) => return run_start(target, start).await,
        ProcessCommand::Logs { id, from, follow } => {
            let read = (process_id(&id)?, log_from(&from)?);
            return hosted_stream::logs(target, read, follow).await;
        }
        ProcessCommand::Attach { id, from, stdin } => {
            let attach = (process_id(&id)?, log_from(&from)?, stdin);
            return hosted_stream::attach(target, attach).await;
        }
        ProcessCommand::Stdin { id, keep_open } => {
            return hosted_stream::stdin(target, (process_id(&id)?, !keep_open)).await;
        }
        ProcessCommand::List { after } => {
            ProcessRequest::List(after.as_deref().map(process_id).transpose()?)
        }
        ProcessCommand::Inspect { id } => ProcessRequest::Inspect(process_id(&id)?),
        ProcessCommand::Signal { id, signal } => {
            ProcessRequest::Signal(process_id(&id)?, signal_named(&signal)?)
        }
        ProcessCommand::Terminate { id, grace } => {
            ProcessRequest::Terminate(process_id(&id)?, grace_ms(&grace)?)
        }
        ProcessCommand::Delete { id } => ProcessRequest::Delete(process_id(&id)?),
    };
    // Only a read gives up on a target that does not answer in time: a change the target
    // may have applied is never reported as timed out.
    let read_only = matches!(
        request,
        ProcessRequest::List(_) | ProcessRequest::Inspect(_)
    );
    let body = OpenBody::Process(request.clone());
    let answered = async {
        let (opened, (mut reader, writer)) = open_bound(target.0, (target.1, None), body).await?;
        let answer = hosted::call(&mut reader).await?;
        drop(writer);
        Ok((opened, answer))
    };
    let (opened, answer) = if read_only {
        bounded(READ_DEADLINE, answered).await?
    } else {
        answered.await?
    };
    print(&checked((&request, &opened), answer)?)
}

/// The view of an answer, which must be the one `request`, opened under `opened`, asked
/// for: never another Process's or another operation's.
pub(super) fn checked(
    request: (&ProcessRequest, &RequestId),
    (accept, reply): (Option<ProcessAccept>, ProcessReply),
) -> Result<Value, AppError> {
    if !answer::process(request, (accept, &reply)) {
        return Err(AppError::Protocol(
            "the target answered another Process or operation".to_owned(),
        ));
    }
    Ok(hosted_view::reply(
        matches!(request.0, ProcessRequest::Start(_)),
        accept,
        &reply,
    ))
}

async fn run_start(target: Target<'_>, arguments: StartArgs) -> Result<(), AppError> {
    let request = ProcessRequest::Start(process_start(&arguments)?);
    let body = OpenBody::Process(request.clone());
    let request_id = arguments.request_id.as_deref();
    let (opened, (mut reader, writer)) = open_bound(target.0, (target.1, request_id), body).await?;
    let answer = hosted::call(&mut reader).await?;
    drop(writer);
    print(&checked((&request, &opened), answer)?)
}

/// This Runtime's own Processes through the local API.
async fn local(state_dir: &Path, paths: IpcPaths, process: ProcessCommand) -> Result<(), AppError> {
    let (operation, fields) = match process {
        ProcessCommand::List { after } => {
            let after = after.as_deref().map(process_id).transpose()?;
            let fields = [("after".to_owned(), json!(after.map(|id| id.to_string())))];
            ("process_list", fields.into_iter().collect())
        }
        ProcessCommand::Inspect { id } => {
            let fields = [("process_id".to_owned(), json!(process_id(&id)?.to_string()))];
            ("process_inspect", fields.into_iter().collect())
        }
        ProcessCommand::Terminate { id, grace } => {
            let mut fields = request_fields()?;
            fields.insert("process_id".to_owned(), json!(process_id(&id)?.to_string()));
            fields.insert("grace_ms".to_owned(), json!(grace_ms(&grace)?));
            ("process_terminate", fields)
        }
        ProcessCommand::Delete { id } => {
            let mut fields = request_fields()?;
            fields.insert("process_id".to_owned(), json!(process_id(&id)?.to_string()));
            ("process_delete", fields)
        }
        ProcessCommand::Start(_)
        | ProcessCommand::Logs { .. }
        | ProcessCommand::Attach { .. }
        | ProcessCommand::Stdin { .. }
        | ProcessCommand::Signal { .. } => {
            return Err(AppError::Usage(
                "this operation needs --endpoint; only list, inspect, terminate, and delete \
                 act on this Runtime's own Processes",
            ));
        }
    };
    crate::call((state_dir, paths), command(operation, fields)?, false).await
}

fn process_start(arguments: &StartArgs) -> Result<ProcessStart, AppError> {
    let bound = |_| AppError::Usage("the start exceeds a protocol bound");
    let (program, args) = arguments
        .argv
        .split_first()
        .ok_or(AppError::Usage("start requires a PROGRAM after --"))?;
    let mut spec = ProcessSpec::new(Some(program.clone()), args.to_vec(), arguments.cwd.clone())
        .map_err(bound)?;
    for entry in &arguments.env {
        let (name, value) = pair(entry, "--env takes NAME=VALUE")?;
        spec = spec
            .with_env(name, value)
            .map_err(|_| AppError::Usage("--env names must be unique [A-Za-z_][A-Za-z0-9_]*"))?;
    }
    let log_limit = arguments.log_limit.unwrap_or(DEFAULT_LOG_LIMIT);
    let mut start = ProcessStart::new(spec, arguments.stdin, log_limit)
        .map_err(|_| AppError::Usage("--log-limit must be within 1048576..=16777216"))?;
    if let Some(name) = &arguments.name {
        start = start.with_name(name.clone()).map_err(bound)?;
    }
    for entry in &arguments.labels {
        let (key, value) = pair(entry, "--label takes KEY=VALUE")?;
        start = start.with_label(key, value).map_err(bound)?;
    }
    Ok(start)
}

fn pair(entry: &str, usage: &'static str) -> Result<(String, String), AppError> {
    let (key, value) = entry.split_once('=').ok_or(AppError::Usage(usage))?;
    Ok((key.to_owned(), value.to_owned()))
}

pub(super) fn process_id(text: &str) -> Result<ProcessId, AppError> {
    ProcessId::parse(text)
        .map_err(|_| AppError::Invalid("PROCESS_ID must be 32 lowercase hex characters".to_owned()))
}

fn log_from(text: &str) -> Result<LogFrom, AppError> {
    match text {
        "start" => Ok(LogFrom::Start),
        "now" => Ok(LogFrom::Now),
        cursor => cursor
            .parse()
            .map(LogFrom::Cursor)
            .map_err(|_| AppError::Usage("--from takes start, now, or a cursor")),
    }
}

fn grace_ms(text: &str) -> Result<u32, AppError> {
    // A zero grace period kills at once, as Process v1 allows.
    let grace = duration_within(text, 0..=u64::from(MAX_TERMINATE_GRACE_MS))?;
    u32::try_from(grace).map_err(|_| AppError::Usage("--grace is at most 60s"))
}

/// Accepts the wire names and the conventional `INT`, `SIGINT`, and similar spellings.
fn signal_named(text: &str) -> Result<ProcessSignal, AppError> {
    let upper = text.to_ascii_uppercase();
    let short = upper.strip_prefix("SIG").unwrap_or(&upper);
    let name = match short {
        "INT" => "interrupt",
        "TERM" => "terminate",
        "KILL" => "kill",
        "HUP" => "hangup",
        "QUIT" => "quit",
        "USR1" => "user1",
        "USR2" => "user2",
        _ => text,
    };
    ProcessSignal::from_name(&name.to_ascii_lowercase()).ok_or(AppError::Usage(
        "SIGNAL is one of interrupt, terminate, kill, hangup, quit, user1, user2",
    ))
}

pub(super) fn print(value: &Value) -> Result<(), AppError> {
    writeln!(std::io::stdout().lock(), "{value}")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{LogFrom, ProcessSignal};

    use super::{grace_ms, log_from, signal_named};

    #[test]
    fn signals_and_positions_take_their_conventional_spellings() {
        assert_eq!(signal_named("SIGTERM").ok(), Some(ProcessSignal::Terminate));
        assert_eq!(signal_named("hup").ok(), Some(ProcessSignal::Hangup));
        assert_eq!(signal_named("user2").ok(), Some(ProcessSignal::User2));
        assert!(signal_named("SIGSEGV").is_err());
        assert_eq!(log_from("start").ok(), Some(LogFrom::Start));
        assert_eq!(log_from("42").ok(), Some(LogFrom::Cursor(42)));
        assert!(log_from("-1").is_err());
    }

    #[test]
    fn a_grace_period_may_be_zero_for_an_immediate_kill() {
        assert_eq!(grace_ms("0s").ok(), Some(0));
        assert_eq!(grace_ms("0ms").ok(), Some(0));
        assert_eq!(grace_ms("60s").ok(), Some(60_000));
        assert!(grace_ms("61s").is_err());
    }
}
