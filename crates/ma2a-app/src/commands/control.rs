use std::{future::Future, io, pin::Pin};

use ma2a_runtime::api::{Command, CommandResult};
use ma2a_runtime::current_user::CurrentUserRuntime;

pub(crate) type ControlFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CommandResult, CurrentUserControlError>> + Send + 'a>>;

pub(crate) trait CurrentUserControlClient: Send {
    fn send(&mut self, command: Command) -> ControlFuture<'_>;
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeControlClient {
    runtime: CurrentUserRuntime,
}

impl RuntimeControlClient {
    pub(crate) const fn new(runtime: CurrentUserRuntime) -> Self {
        Self { runtime }
    }
}

impl CurrentUserControlClient for RuntimeControlClient {
    fn send(&mut self, command: Command) -> ControlFuture<'_> {
        Box::pin(async move {
            self.runtime
                .send(command)
                .await
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
