use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    api::{Command, CommandResult, UiAuthView, UiControlCommand},
    web::{Clock, PasswordAction, SystemClock, WebAuthConfig, WebAuthService},
};

/// Runtime-owned current-user composition used by trusted CLI control and Web serving.
#[derive(Clone, Debug)]
pub struct CurrentUserRuntime {
    auth: WebAuthService,
}

/// Closed failures from current-user Runtime composition and UI control.
#[derive(Debug)]
#[non_exhaustive]
pub enum CurrentUserError {
    /// No absolute current-user state root is available from the operating system environment.
    Environment,
    /// A non-UI command reached the UI-only control boundary.
    InvalidCommand,
    /// Credential or session state failed closed.
    Authentication(crate::web::AuthFailure),
}

impl CurrentUserRuntime {
    /// Opens the Runtime at the platform current-user state directory.
    ///
    /// # Errors
    /// Returns [`CurrentUserError`] when the state root or authentication store is unavailable.
    pub async fn open() -> Result<Self, CurrentUserError> {
        let state_dir = current_user_state_dir()?;
        Self::open_at(&state_dir, Arc::new(SystemClock), WebAuthConfig::default()).await
    }

    /// Opens the Runtime at an explicit protected state directory.
    ///
    /// # Errors
    /// Returns [`CurrentUserError`] when the authentication store is unavailable.
    pub async fn open_at(
        state_dir: impl AsRef<Path>,
        clock: Arc<dyn Clock>,
        config: WebAuthConfig,
    ) -> Result<Self, CurrentUserError> {
        let auth = WebAuthService::open(ma2a_store::StoreConfig::new(state_dir), clock, config)
            .await
            .map_err(CurrentUserError::Authentication)?;
        Ok(Self { auth })
    }

    /// Executes one typed UI credential command.
    ///
    /// # Errors
    /// Returns [`CurrentUserError`] for non-UI commands or failed credential/session mutations.
    pub async fn send(&self, command: Command) -> Result<CommandResult, CurrentUserError> {
        match command
            .into_ui_control()
            .map_err(|_| CurrentUserError::InvalidCommand)?
        {
            UiControlCommand::PasswordInit(password) => {
                let committed = self
                    .auth
                    .change_password(PasswordAction::Init, password)
                    .await
                    .map_err(CurrentUserError::Authentication)?;
                Ok(
                    CommandResult::ui_initialized(UiAuthView::new(true, true, 0))
                        .at_revision(committed.revision),
                )
            }
            UiControlCommand::PasswordSet(password) => {
                let committed = self
                    .auth
                    .change_password(PasswordAction::Set, password)
                    .await
                    .map_err(CurrentUserError::Authentication)?;
                Ok(
                    CommandResult::ui_password_set(UiAuthView::new(true, true, 0))
                        .at_revision(committed.revision),
                )
            }
            UiControlCommand::PasswordReset(password) => {
                let committed = self
                    .auth
                    .change_password(PasswordAction::Reset, password)
                    .await
                    .map_err(CurrentUserError::Authentication)?;
                Ok(
                    CommandResult::ui_password_reset(UiAuthView::new(true, true, 0))
                        .at_revision(committed.revision),
                )
            }
            UiControlCommand::SessionsRevokeAll => {
                let committed = self
                    .auth
                    .revoke_all_sessions_committed()
                    .await
                    .map_err(CurrentUserError::Authentication)?;
                Ok(
                    CommandResult::sessions_revoked(UiAuthView::new(true, *committed.value(), 0))
                        .at_revision(committed.revision()),
                )
            }
        }
    }

    /// Returns the shared Web authentication service owned by this Runtime.
    #[must_use]
    pub const fn web_auth(&self) -> &WebAuthService {
        &self.auth
    }
}

impl fmt::Display for CurrentUserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment => formatter.write_str("current-user state directory is unavailable"),
            Self::InvalidCommand => formatter.write_str("command is not valid for UI control"),
            Self::Authentication(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CurrentUserError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Authentication(error) => Some(error),
            Self::Environment | Self::InvalidCommand => None,
        }
    }
}

fn current_user_state_dir() -> Result<PathBuf, CurrentUserError> {
    #[cfg(unix)]
    {
        unix_state_dir(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"))
    }
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("ma2a"))
            .ok_or(CurrentUserError::Environment)
    }
}

#[cfg(unix)]
fn unix_state_dir(
    xdg_state_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<PathBuf, CurrentUserError> {
    if let Some(path) = xdg_state_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Ok(path.join("ma2a"));
    }
    home.map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(".local/state/ma2a"))
        .ok_or(CurrentUserError::Environment)
}
