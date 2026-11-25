use std::{error::Error, fmt, io};

use ma2a_runtime::{RuntimeError, current_user::CurrentUserError, ipc::IpcError};

use crate::commands;

pub(crate) enum AppError {
    Command(commands::ui::UiCommandError),
    CurrentUser(CurrentUserError),
    /// A lifecycle action refused to act on a daemon it could not safely act on.
    Daemon(String),
    /// Ownership of the state directory and what answers the endpoint disagree.
    DaemonConflicted(String),
    DaemonStopped,
    /// A caller-supplied reference the CLI could not turn into one object.
    Invalid(String),
    Io(io::Error),
    Ipc(IpcError),
    /// The named object does not exist for this Endpoint.
    NotFound(String),
    /// A typed Runtime protocol error, surfaced with its real name.
    Protocol(String),
    /// The Runtime refused a request: its error name, and the message shown for it.
    Rejected(String, String),
    Runtime(RuntimeError),
    Usage(&'static str),
}

impl AppError {
    /// The status the CLI exits with for this error.
    pub(crate) const fn exit_code(&self) -> u8 {
        match self {
            Self::Invalid(_) | Self::Usage(_) => 2,
            Self::Command(_)
            | Self::CurrentUser(_)
            | Self::Daemon(_)
            | Self::DaemonConflicted(_)
            | Self::DaemonStopped
            | Self::Io(_)
            | Self::Ipc(_)
            | Self::NotFound(_)
            | Self::Protocol(_)
            | Self::Rejected(..)
            | Self::Runtime(_) => 1,
        }
    }
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
            Self::Daemon(message)
            | Self::DaemonConflicted(message)
            | Self::Invalid(message)
            | Self::NotFound(message)
            | Self::Protocol(message)
            | Self::Rejected(_, message) => formatter.write_str(message),
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
            Self::Daemon(_)
            | Self::DaemonConflicted(_)
            | Self::DaemonStopped
            | Self::Invalid(_)
            | Self::NotFound(_)
            | Self::Protocol(_)
            | Self::Rejected(..)
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
