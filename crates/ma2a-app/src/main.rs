//! Installed MA2A command and singleton daemon entry point.

use std::{
    error::Error,
    fmt::{self, Write as _},
    io,
    path::PathBuf,
    process::ExitCode,
};

use ma2a_core::RequestId;
use ma2a_runtime::{
    RuntimeError, api,
    current_user::CurrentUserError,
    ipc::{IpcError, IpcPaths, LocalApiClient},
};
mod cli;
mod commands;
mod credential_command;
mod daemon;
mod daemon_control;
mod output;

mod embedded_web {
    include!(concat!(env!("OUT_DIR"), "/embedded_web.rs"));
}

enum AppError {
    Command(commands::ui::UiCommandError),
    CurrentUser(CurrentUserError),
    DaemonStopped,
    /// A caller-supplied reference the CLI could not turn into one object.
    Invalid(String),
    Io(io::Error),
    Ipc(IpcError),
    /// The named object does not exist for this Endpoint.
    NotFound(String),
    /// A typed Runtime protocol error, surfaced with its real name.
    Protocol(String),
    Runtime(RuntimeError),
    Usage(&'static str),
}

impl fmt::Debug for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(error) => error.fmt(formatter),
            Self::CurrentUser(error) => error.fmt(formatter),
            Self::DaemonStopped => formatter.write_str(
                "daemon is not running; run `ma2a start` with the same --state-dir first",
            ),
            Self::Invalid(message) | Self::NotFound(message) | Self::Protocol(message) => {
                formatter.write_str(message)
            }
            Self::Io(error) => write!(formatter, "MA2A I/O failed: {error}"),
            Self::Ipc(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Usage(message) => formatter.write_str(message),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Command(error) => Some(error),
            Self::CurrentUser(error) => Some(error),
            Self::DaemonStopped
            | Self::Invalid(_)
            | Self::NotFound(_)
            | Self::Protocol(_)
            | Self::Usage(_) => None,
            Self::Io(error) => Some(error),
            Self::Ipc(error) => Some(error),
            Self::Runtime(error) => Some(error),
        }
    }
}

impl From<io::Error> for AppError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<IpcError> for AppError {
    fn from(error: IpcError) -> Self {
        Self::Ipc(error)
    }
}

impl From<RuntimeError> for AppError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(error)
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> ExitCode {
    std::hint::black_box(embedded_web::WEB_ASSETS);
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let parsed = cli::Cli::parse_safe(arguments);
    let cli = match parsed {
        Ok(cli) => cli,
        Err(error) if error.kind() == clap::error::ErrorKind::InvalidSubcommand => {
            eprintln!("unknown command; run ma2a --help");
            return ExitCode::from(2);
        }
        Err(error) => {
            let code = if error.use_stderr() { 2 } else { 0 };
            let _rendered = error.print();
            return ExitCode::from(code);
        }
    };
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            match error {
                AppError::Invalid(_) | AppError::Usage(_) => ExitCode::from(2),
                AppError::Command(_)
                | AppError::CurrentUser(_)
                | AppError::DaemonStopped
                | AppError::Io(_)
                | AppError::Ipc(_)
                | AppError::NotFound(_)
                | AppError::Protocol(_)
                | AppError::Runtime(_) => ExitCode::FAILURE,
            }
        }
    }
}

async fn run(cli: cli::Cli) -> Result<(), AppError> {
    let state_dir = match cli.state_dir {
        Some(path) => path,
        None => default_state_dir()?,
    };
    let paths = IpcPaths::new(&state_dir)?;
    match cli.command {
        cli::Command::Start => {
            let outcome = daemon_control::start(&state_dir, &paths).await?;
            match outcome {
                daemon_control::StartOutcome::Started => println!("daemon started"),
                daemon_control::StartOutcome::AlreadyRunning => println!("daemon already running"),
            }
            Ok(())
        }
        cli::Command::Restart => {
            daemon_control::restart(&state_dir, &paths).await?;
            println!("daemon restarted");
            Ok(())
        }
        cli::Command::Stop => {
            daemon_control::stop(&paths).await?;
            println!("daemon stopped");
            Ok(())
        }
        cli::Command::Daemon => daemon::run(state_dir, paths, false).await,
        cli::Command::DaemonDetached => daemon::run(state_dir, paths, true).await,
        cli::Command::Status { json } => {
            call((&state_dir, paths), api::Command::snapshot_fetch(), json).await
        }
        cli::Command::Endpoint { command } => match command {
            cli::EndpointCommand::Show { json } => {
                call(
                    (&state_dir, paths),
                    commands::workflows::unit_command("endpoint_info")?,
                    json,
                )
                .await
            }
        },
        cli::Command::Space { command } => {
            commands::workflows::run_space(&state_dir, paths, command).await
        }
        cli::Command::Relay { command } => {
            commands::workflows::run_relay(&state_dir, paths, command).await
        }
        cli::Command::Echo(arguments) => {
            commands::workflows::run_echo(&state_dir, paths, arguments).await
        }
        cli::Command::Ui { command } => commands::workflows::run_ui(&state_dir, command).await,
    }
}

/// Sends one command to a running daemon and returns its raw response frame.
async fn request(
    runtime: (&std::path::Path, IpcPaths),
    command: api::Command,
) -> Result<Vec<u8>, AppError> {
    let (_, paths) = runtime;
    daemon_control::require(&paths).await?;
    Ok(LocalApiClient::new(paths).call(&command).await?)
}

async fn call(
    runtime: (&std::path::Path, IpcPaths),
    command: api::Command,
    json: bool,
) -> Result<(), AppError> {
    let response = request(runtime, command).await?;
    let document: serde_json::Value =
        serde_json::from_slice(&response).map_err(io::Error::other)?;
    // A Runtime error envelope carries no result, so it is reported as the typed
    // protocol error it is instead of being parsed as human output.
    let outcome = output::reject_runtime_error(&document);
    if json {
        output::write_json(&response)?;
    } else if outcome.is_ok() {
        output::write_human(&document)?;
    }
    outcome
}

async fn stop_daemon(paths: IpcPaths) -> Result<(), AppError> {
    daemon_control::require(&paths).await?;
    let command = shutdown_command()?;
    let response = LocalApiClient::new(paths.clone()).call(&command).await?;
    let document: serde_json::Value =
        serde_json::from_slice(&response).map_err(io::Error::other)?;
    if document
        .pointer("/result/type")
        .and_then(serde_json::Value::as_str)
        != Some("shutting_down")
    {
        return Err(io::Error::other("daemon rejected the stop request").into());
    }
    daemon_control::wait_until_stopped(&paths).await?;
    Ok(())
}

fn shutdown_command() -> Result<api::Command, AppError> {
    let request_id = RequestId::random()
        .map_err(|_| io::Error::other("operating-system random source failed"))?;
    let mut encoded_id = String::with_capacity(32);
    for byte in request_id.as_bytes() {
        write!(&mut encoded_id, "{byte:02x}")
            .map_err(|_| io::Error::other("request identifier encoding failed"))?;
    }
    let request = format!(
        r#"{{"version":{},"operation":"graceful_shutdown","request_id":"{encoded_id}"}}"#,
        api::LOCAL_API_VERSION
    );
    Ok(api::decode_command(request.as_bytes()).map_err(IpcError::from)?)
}

fn default_state_dir() -> Result<PathBuf, AppError> {
    #[cfg(windows)]
    let root = PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or(AppError::Usage("LOCALAPPDATA is not set"))?,
    );
    #[cfg(unix)]
    let root = match std::env::var_os("XDG_STATE_HOME") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(std::env::var_os("HOME").ok_or(AppError::Usage("HOME is not set"))?)
            .join(".local/state"),
    };
    Ok(root.join("ma2a"))
}

#[cfg(test)]
mod tests {
    use super::{embedded_web, shutdown_command};

    #[test]
    fn embedded_frontend_is_present_when_binary_is_built() {
        assert!(!embedded_web::WEB_ASSETS.is_empty());
    }

    #[test]
    fn shutdown_commands_use_fresh_request_identifiers() {
        // Given
        let first = shutdown_command().expect("first shutdown command");

        // When
        let second = shutdown_command().expect("second shutdown command");

        // Then
        assert_ne!(first.request_id(), second.request_id());
    }
}
