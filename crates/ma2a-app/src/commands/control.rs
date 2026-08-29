use std::{future::Future, io, path::PathBuf, pin::Pin};

use ma2a_runtime::{
    api::{Command, CommandResult, decode_ui_control_response},
    ipc::{IpcPaths, LocalApiClient},
};

pub(crate) type ControlFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CommandResult, CurrentUserControlError>> + Send + 'a>>;

pub(crate) trait CurrentUserControlClient: Send {
    fn send(&mut self, command: Command) -> ControlFuture<'_>;
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeControlClient {
    state_dir: PathBuf,
    paths: IpcPaths,
    client: LocalApiClient,
}

impl RuntimeControlClient {
    pub(crate) fn new(state_dir: PathBuf, paths: IpcPaths) -> Self {
        Self {
            state_dir,
            client: LocalApiClient::new(paths.clone()),
            paths,
        }
    }
}

impl CurrentUserControlClient for RuntimeControlClient {
    fn send(&mut self, command: Command) -> ControlFuture<'_> {
        Box::pin(async move {
            crate::autostart::ensure_daemon(&self.state_dir, &self.paths)
                .await
                .map_err(|error| CurrentUserControlError::from(io::Error::other(error)))?;
            let response = self
                .client
                .call(&command)
                .await
                .map_err(|error| CurrentUserControlError::from(io::Error::other(error)))?;
            decode_ui_control_response(&command, &response)
                .map_err(|error| CurrentUserControlError::from(io::Error::other(error)))
        })
    }
}

#[derive(Debug)]
pub(crate) struct CurrentUserControlError(io::Error);

impl From<io::Error> for CurrentUserControlError {
    fn from(error: io::Error) -> Self {
        Self(error)
    }
}

impl std::fmt::Display for CurrentUserControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "current-user control transport failed: {}",
            self.0
        )
    }
}

impl std::error::Error for CurrentUserControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
