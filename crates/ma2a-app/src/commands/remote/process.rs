//! `ma2a exec` and `ma2a shell`: remote programs bound to local pipes or this terminal.

use std::io;

use ma2a_core::service::{
    EndReason, ExecRequest, MAX_OPERATION_LIFETIME_MS, OpenBody, ProcessResult, ProcessSpec,
    ShellRequest, TerminalSize, Termination,
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client::{
    self,
    process::{ProcessControl, ProcessIo},
    terminal,
};

use super::{
    duration_ms, open,
    stdio::{StdioGuard, interrupted_io},
};
use crate::{
    AppError,
    cli::services::{ExecArgs, ProgramArgs, ShellArgs},
};

/// Terminal type sent when the local one is unset or unusable.
const DEFAULT_TERM: &str = "xterm-256color";

pub(crate) async fn exec(paths: IpcPaths, arguments: ExecArgs) -> Result<(), AppError> {
    let (program, args) = arguments
        .argv
        .split_first()
        .ok_or(AppError::Usage("exec requires a PROGRAM after --"))?;
    let spec = spec(&arguments.program, Some(program.clone()), args.to_vec())?;
    let timeout = duration_ms(&arguments.timeout, MAX_OPERATION_LIFETIME_MS)?;
    let request = ExecRequest::new(spec, arguments.stdin, timeout)
        .map_err(|_| AppError::Usage("exec request exceeds a protocol bound"))?;
    let (mut reader, mut writer) =
        open(&paths, &arguments.program.endpoint, OpenBody::Exec(request)).await?;
    let stdio = StdioGuard::default();
    let local = ProcessIo {
        input: arguments.stdin.then(tokio::io::stdin),
        output: stdio.stdout(),
        error: Some(stdio.stderr()),
    };
    let control = ProcessControl {
        resizes: None,
        cancel: interrupted(),
    };
    match client::process::run_streamed((&mut reader, &mut writer), local, control).await {
        Ok(streamed) => {
            if streamed.delivery_interrupted && streamed.result.reason == EndReason::Completed {
                stdio.exit_error(interrupted_io().into()).await;
            }
            if streamed.replayed {
                stdio
                    .note_stderr(
                        "note: this request already ran; its outcome was replayed, and its output was not retained",
                    )
                    .await;
            }
            exit_with(&stdio, &streamed.result).await
        }
        Err(error) => stdio.exit_error(error.into()).await,
    }
}

pub(crate) async fn shell(paths: IpcPaths, arguments: ShellArgs) -> Result<(), AppError> {
    let (program, args) = match arguments.argv.split_first() {
        Some((program, args)) => (Some(program.clone()), args.to_vec()),
        None => (None, Vec::new()),
    };
    let spec = spec(&arguments.program, program, args)?;
    let lifetime = duration_ms(&arguments.lifetime, MAX_OPERATION_LIFETIME_MS)?;
    let size = match terminal::size() {
        Some(size) => size,
        None => TerminalSize::new(80, 24).map_err(|_| io::Error::other("terminal size"))?,
    };
    let term = std::env::var("TERM")
        .ok()
        .filter(|term| !term.is_empty() && term.bytes().all(|byte| byte.is_ascii_graphic()))
        .unwrap_or_else(|| DEFAULT_TERM.to_owned());
    let request = ShellRequest::new(spec, (term, size), lifetime)
        .map_err(|_| AppError::Usage("shell request exceeds a protocol bound"))?;
    let (mut reader, mut writer) = open(
        &paths,
        &arguments.program.endpoint,
        OpenBody::Shell(request),
    )
    .await?;
    // Raw mode starts only once the session exists, so every earlier error prints normally.
    let raw = terminal::RawMode::enable()?;
    let stdio = StdioGuard::default();
    let local = ProcessIo::<_, _, tokio::io::Stderr> {
        input: Some(tokio::io::stdin()),
        output: stdio.stdout(),
        error: None,
    };
    let control = ProcessControl {
        resizes: terminal::resizes().ok(),
        // In raw mode Ctrl-C is terminal input for the remote program, not a local cancel.
        cancel: std::future::pending(),
    };
    let result = client::process::run((&mut reader, &mut writer), local, control).await;
    drop(raw);
    match result {
        Ok(result) => exit_with(&stdio, &result).await,
        Err(error) => stdio.exit_error(error.into()).await,
    }
}

fn spec(
    arguments: &ProgramArgs,
    program: Option<String>,
    args: Vec<String>,
) -> Result<ProcessSpec, AppError> {
    let bound = |_| AppError::Usage("program, arguments, or cwd exceed a protocol bound");
    let mut spec = ProcessSpec::new(program, args, arguments.cwd.clone()).map_err(bound)?;
    for entry in &arguments.env {
        let (name, value) = entry
            .split_once('=')
            .ok_or(AppError::Usage("--env takes NAME=VALUE"))?;
        spec = spec
            .with_env(name.to_owned(), value.to_owned())
            .map_err(|_| AppError::Usage("--env names must be unique [A-Za-z_][A-Za-z0-9_]*"))?;
    }
    Ok(spec)
}

/// Completes on the first Ctrl-C; never, when the signal cannot be observed.
pub(super) async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Exits with the remote program's status, as a local program would.
///
/// The process exits directly because a pending read of stdin or a blocked stdio worker cannot
/// be cancelled and would otherwise hold the runtime open.
async fn exit_with(stdio: &StdioGuard, result: &ProcessResult) -> ! {
    let code = match (result.reason, result.termination) {
        (EndReason::TimedOut, _) => {
            stdio.note_stderr("remote program timed out").await;
            124
        }
        (EndReason::Cancelled, _) => 130,
        (EndReason::Shutdown, _) => {
            stdio.note_stderr("target Runtime shut down").await;
            255
        }
        (_, Termination::Exited(code)) => i32::try_from(code.min(255)).unwrap_or(255),
        (_, Termination::Signaled(signal)) => i32::try_from(128 + signal.min(127)).unwrap_or(255),
        (_, _) => 255,
    };
    stdio
        .exit_code(code, result.reason == EndReason::Completed)
        .await
}
