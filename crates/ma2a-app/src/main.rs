//! Installed MA2A command and singleton daemon entry point.

use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt::{self, Write as _},
    io::{self, Write as _},
    path::PathBuf,
    sync::Arc,
};

use ma2a_core::RequestId;
use ma2a_runtime::{
    RuntimeError, api,
    current_user::{CurrentUserError, CurrentUserRuntime},
    ipc::{IpcError, IpcPaths, LocalApiClient},
    web::{LoopbackWebServer, SystemClock, WebAssets, WebAuthConfig, WebServerConfig},
};

mod autostart;
mod commands;
mod daemon;

use commands::{
    control::RuntimeControlClient,
    ui::{PasswordCommand, TerminalPasswordReader},
};
mod embedded_web {
    include!(concat!(env!("OUT_DIR"), "/embedded_web.rs"));
}

enum AppError {
    Command(commands::ui::UiCommandError),
    CurrentUser(CurrentUserError),
    Io(io::Error),
    Ipc(IpcError),
    Runtime(RuntimeError),
    Usage(&'static str),
    Web(io::Error),
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
            Self::Io(error) => write!(formatter, "MA2A I/O failed: {error}"),
            Self::Ipc(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Usage(message) => formatter.write_str(message),
            Self::Web(error) => write!(formatter, "loopback Web server failed: {error}"),
        }
    }
}

impl Error for AppError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Command(error) => Some(error),
            Self::CurrentUser(error) => Some(error),
            Self::Io(error) | Self::Web(error) => Some(error),
            Self::Ipc(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Usage(_) => None,
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

enum Command {
    Daemon,
    DaemonDetached,
    Init,
    Status,
    Shutdown,
    UiPasswordSet,
    UiPasswordReset,
    UiSessionRevokeAll,
    Web,
    Help,
    Version,
}

struct Cli {
    state_dir: PathBuf,
    command: Command,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<(), AppError> {
    std::hint::black_box(embedded_web::WEB_ASSETS);
    let cli = parse_args(std::env::args_os().skip(1))?;
    let paths = IpcPaths::new(&cli.state_dir)?;
    match cli.command {
        Command::Daemon => daemon::run(cli.state_dir, paths, false).await,
        Command::DaemonDetached => daemon::run(cli.state_dir, paths, true).await,
        Command::Init | Command::UiPasswordSet => {
            run_password_at(&cli.state_dir, PasswordCommand::Set).await
        }
        Command::UiPasswordReset => run_password_at(&cli.state_dir, PasswordCommand::Reset).await,
        Command::UiSessionRevokeAll => run_revoke_all_at(&cli.state_dir).await,
        Command::Web => run_web_at(&cli.state_dir).await,
        Command::Status => {
            call(
                &cli.state_dir,
                paths,
                br#"{"version":1,"operation":"status"}"#,
            )
            .await
        }
        Command::Shutdown => call_shutdown(paths).await,
        Command::Help => write_help(),
        Command::Version => {
            writeln!(io::stdout().lock(), "ma2a {}", env!("CARGO_PKG_VERSION"))?;
            Ok(())
        }
    }
}

async fn call(
    state_dir: &std::path::Path,
    paths: IpcPaths,
    request: &[u8],
) -> Result<(), AppError> {
    autostart::ensure_daemon(state_dir, &paths).await?;
    let command = api::decode_command(request).map_err(IpcError::from)?;
    let response = LocalApiClient::new(paths).call(&command).await?;
    io::stdout().lock().write_all(&response)?;
    writeln!(io::stdout().lock())?;
    Ok(())
}

async fn call_shutdown(paths: IpcPaths) -> Result<(), AppError> {
    let command = shutdown_command()?;
    let response = LocalApiClient::new(paths.clone()).call(&command).await?;
    autostart::wait_until_stopped(&paths).await?;
    io::stdout().lock().write_all(&response)?;
    writeln!(io::stdout().lock())?;
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
    let request =
        format!(r#"{{"version":1,"operation":"graceful_shutdown","request_id":"{encoded_id}"}}"#);
    Ok(api::decode_command(request.as_bytes()).map_err(IpcError::from)?)
}

fn parse_args(arguments: impl Iterator<Item = OsString>) -> Result<Cli, AppError> {
    let arguments = arguments.collect::<Vec<_>>();
    let (state_dir, command_arguments) = match arguments.as_slice() {
        [flag, state_dir, command_arguments @ ..] if flag == OsStr::new("--state-dir") => {
            (PathBuf::from(state_dir), command_arguments)
        }
        [flag] if flag == OsStr::new("--state-dir") => {
            return Err(AppError::Usage("--state-dir requires a path"));
        }
        _ => (default_state_dir()?, arguments.as_slice()),
    };
    let command = match command_arguments {
        [] => Command::Help,
        [value] if value == OsStr::new("daemon") => Command::Daemon,
        [value] if value == OsStr::new("daemon-detached") => Command::DaemonDetached,
        [value] if value == OsStr::new("init") => Command::Init,
        [value] if value == OsStr::new("status") => Command::Status,
        [value] if value == OsStr::new("shutdown") => Command::Shutdown,
        [value] if value == OsStr::new("web") => Command::Web,
        [value] if value == OsStr::new("--help") || value == OsStr::new("-h") => Command::Help,
        [value] if value == OsStr::new("--version") || value == OsStr::new("-V") => {
            Command::Version
        }
        [ui, password, action]
            if ui == OsStr::new("ui")
                && password == OsStr::new("password")
                && action == OsStr::new("set") =>
        {
            Command::UiPasswordSet
        }
        [ui, password, action]
            if ui == OsStr::new("ui")
                && password == OsStr::new("password")
                && action == OsStr::new("reset") =>
        {
            Command::UiPasswordReset
        }
        [ui, session, action]
            if ui == OsStr::new("ui")
                && session == OsStr::new("session")
                && action == OsStr::new("revoke-all") =>
        {
            Command::UiSessionRevokeAll
        }
        _ => return Err(AppError::Usage("unknown command; run ma2a --help")),
    };
    Ok(Cli { state_dir, command })
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

fn write_help() -> Result<(), AppError> {
    writeln!(
        io::stdout().lock(),
        "Usage:\n  ma2a [--state-dir PATH] <daemon|status|shutdown|web|init>\n  ma2a [--state-dir PATH] ui password <set|reset>\n  ma2a [--state-dir PATH] ui session revoke-all\n  ma2a --help\n  ma2a --version"
    )?;
    Ok(())
}

async fn run_password_at(
    state_dir: &std::path::Path,
    action: PasswordCommand,
) -> Result<(), AppError> {
    let runtime = CurrentUserRuntime::open_at(
        state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let mut client = RuntimeControlClient::new(runtime);
    let mut reader = TerminalPasswordReader;
    commands::ui::change_password(action, &mut reader, &mut client)
        .await
        .map_err(AppError::Command)?;
    writeln!(io::stdout().lock(), "Web password updated").map_err(AppError::Io)
}

async fn run_revoke_all_at(state_dir: &std::path::Path) -> Result<(), AppError> {
    let runtime = CurrentUserRuntime::open_at(
        state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let mut client = RuntimeControlClient::new(runtime);
    commands::ui::revoke_all_sessions(&mut client)
        .await
        .map_err(AppError::Command)?;
    writeln!(io::stdout().lock(), "All Web sessions revoked").map_err(AppError::Io)
}

async fn run_web_at(state_dir: &std::path::Path) -> Result<(), AppError> {
    let runtime = CurrentUserRuntime::open_at(
        state_dir,
        Arc::new(SystemClock::default()),
        WebAuthConfig::default(),
    )
    .await
    .map_err(AppError::CurrentUser)?;
    let server = LoopbackWebServer::bind(
        runtime.web_auth().clone(),
        WebAssets::new(embedded_web::WEB_ASSETS),
        WebServerConfig::default(),
    )
    .await
    .map_err(AppError::Web)?;
    writeln!(
        io::stdout().lock(),
        "MA2A Web: http://127.0.0.1:{}",
        server.port()
    )
    .map_err(AppError::Io)?;
    server
        .serve(async {
            let _result = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(AppError::Web)
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
