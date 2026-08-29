//! Installed MA2A command and singleton daemon entry point.

use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt::{self, Write as _},
    io::{self, Write as _},
    path::PathBuf,
};

use ma2a_core::RequestId;
use ma2a_runtime::{
    RuntimeError, api,
    ipc::{IpcError, IpcPaths, LocalApiClient},
};

mod autostart;
mod daemon;

mod embedded_web {
    include!(concat!(env!("OUT_DIR"), "/embedded_web.rs"));
}

enum AppError {
    Io(io::Error),
    Ipc(IpcError),
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
            Self::Io(error) => write!(formatter, "MA2A I/O failed: {error}"),
            Self::Ipc(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Usage(message) => formatter.write_str(message),
        }
    }
}

impl Error for AppError {}

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
    Status,
    Shutdown,
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

fn parse_args(mut arguments: impl Iterator<Item = OsString>) -> Result<Cli, AppError> {
    let first = arguments.next();
    let (state_dir, command) = if first.as_deref() == Some(OsStr::new("--state-dir")) {
        let state_dir = arguments
            .next()
            .ok_or(AppError::Usage("--state-dir requires a path"))?;
        let command = arguments.next();
        (PathBuf::from(state_dir), command)
    } else {
        (default_state_dir()?, first)
    };
    if arguments.next().is_some() {
        return Err(AppError::Usage(
            "unexpected extra argument; run ma2a --help",
        ));
    }
    let command = match command.as_deref() {
        Some(value) if value == OsStr::new("daemon") => Command::Daemon,
        Some(value) if value == OsStr::new("daemon-detached") => Command::DaemonDetached,
        Some(value) if value == OsStr::new("status") => Command::Status,
        Some(value) if value == OsStr::new("shutdown") => Command::Shutdown,
        Some(value) if value == OsStr::new("--help") || value == OsStr::new("-h") => Command::Help,
        Some(value) if value == OsStr::new("--version") || value == OsStr::new("-V") => {
            Command::Version
        }
        None => Command::Help,
        Some(_) => return Err(AppError::Usage("unknown command; run ma2a --help")),
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
        "Usage: ma2a [--state-dir PATH] <daemon|status|shutdown>\n       ma2a --help\n       ma2a --version"
    )?;
    Ok(())
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
