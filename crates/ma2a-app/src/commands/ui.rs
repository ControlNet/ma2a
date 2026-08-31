use std::fmt;

use ma2a_runtime::api::{Command, UiControlResult};
use zeroize::Zeroizing;

use super::control::{CurrentUserControlClient, CurrentUserControlError};

pub(crate) trait PasswordReader: Send {
    fn read(&mut self, prompt: &str) -> std::io::Result<Zeroizing<String>>;
}

#[derive(Debug, Default)]
pub(crate) struct TerminalPasswordReader;

impl PasswordReader for TerminalPasswordReader {
    fn read(&mut self, prompt: &str) -> std::io::Result<Zeroizing<String>> {
        rpassword::prompt_password(prompt).map(Zeroizing::new)
    }
}

pub(crate) struct InheritedStdinPasswordReader<R> {
    input: R,
}

impl<R> InheritedStdinPasswordReader<R> {
    pub(crate) const fn new(input: R) -> Self {
        Self { input }
    }
}

impl<R: std::io::BufRead + Send> PasswordReader for InheritedStdinPasswordReader<R> {
    fn read(&mut self, _prompt: &str) -> std::io::Result<Zeroizing<String>> {
        let mut value = Zeroizing::new(String::new());
        if self.input.read_line(&mut value)? == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "password input ended before a newline-delimited value",
            ));
        }
        if value.ends_with('\n') {
            value.pop();
            if value.ends_with('\r') {
                value.pop();
            }
        }
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PasswordCommand {
    Set,
    Reset,
}

#[derive(Debug)]
pub(crate) enum UiCommandError {
    ConfirmationMismatch,
    Control(CurrentUserControlError),
    InvalidCommand(ma2a_runtime::api::ApiError),
    Read(std::io::Error),
    UnexpectedResult,
}

impl fmt::Display for UiCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfirmationMismatch => {
                formatter.write_str("password confirmation did not match")
            }
            Self::Control(error) => error.fmt(formatter),
            Self::InvalidCommand(error) => error.fmt(formatter),
            Self::Read(error) => write!(formatter, "failed to read password without echo: {error}"),
            Self::UnexpectedResult => {
                formatter.write_str("Runtime returned an unexpected local-control result")
            }
        }
    }
}

impl std::error::Error for UiCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Control(error) => Some(error),
            Self::InvalidCommand(error) => Some(error),
            Self::Read(error) => Some(error),
            Self::ConfirmationMismatch | Self::UnexpectedResult => None,
        }
    }
}

pub(crate) async fn change_password(
    action: PasswordCommand,
    reader: &mut impl PasswordReader,
    client: &mut dyn CurrentUserControlClient,
) -> Result<(), UiCommandError> {
    let password = reader.read("Password: ").map_err(UiCommandError::Read)?;
    let confirmation = reader
        .read("Confirm password: ")
        .map_err(UiCommandError::Read)?;
    if password.as_bytes() != confirmation.as_bytes() {
        return Err(UiCommandError::ConfirmationMismatch);
    }
    let command = match action {
        PasswordCommand::Set => Command::ui_password_set(password),
        PasswordCommand::Reset => Command::ui_password_reset(password),
    }
    .map_err(UiCommandError::InvalidCommand)?;
    let result = client
        .send(command)
        .await
        .map_err(UiCommandError::Control)?;
    match (action, result.ui_control_result()) {
        (PasswordCommand::Set, Some(UiControlResult::PasswordSet(_)))
        | (PasswordCommand::Reset, Some(UiControlResult::PasswordReset(_))) => Ok(()),
        (PasswordCommand::Set | PasswordCommand::Reset, _) => Err(UiCommandError::UnexpectedResult),
    }
}

pub(crate) async fn revoke_all_sessions(
    client: &mut dyn CurrentUserControlClient,
) -> Result<(), UiCommandError> {
    let command = Command::session_revoke_all().map_err(UiCommandError::InvalidCommand)?;
    let result = client
        .send(command)
        .await
        .map_err(UiCommandError::Control)?;
    match result.ui_control_result() {
        Some(UiControlResult::SessionsRevoked(_)) => Ok(()),
        Some(_) | None => Err(UiCommandError::UnexpectedResult),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io::Cursor};

    use ma2a_runtime::api::{CommandResult, UiAuthView, encode_command};

    use super::*;
    use crate::commands::control::ControlFuture;

    struct InjectedReader {
        values: VecDeque<&'static str>,
    }

    impl PasswordReader for InjectedReader {
        fn read(&mut self, _prompt: &str) -> std::io::Result<Zeroizing<String>> {
            self.values
                .pop_front()
                .map(|value| Zeroizing::new(value.to_owned()))
                .ok_or_else(|| std::io::Error::other("injected password input exhausted"))
        }
    }

    struct RecordingClient {
        commands: Vec<Command>,
        results: VecDeque<CommandResult>,
    }

    #[test]
    fn inherited_stdin_reader_returns_newline_delimited_secrets() -> Result<(), std::io::Error> {
        // Given
        let input = Cursor::new(b"first secret\r\nsecond secret\n".to_vec());
        let mut reader = InheritedStdinPasswordReader::new(input);

        // When
        let first = reader.read("ignored")?;
        let second = reader.read("ignored")?;

        // Then
        assert_eq!(first.as_str(), "first secret");
        assert_eq!(second.as_str(), "second secret");
        Ok(())
    }

    #[test]
    fn inherited_stdin_reader_rejects_premature_eof() {
        // Given
        let input = Cursor::new(Vec::<u8>::new());
        let mut reader = InheritedStdinPasswordReader::new(input);

        // When
        let result = reader.read("ignored");

        // Then
        assert_eq!(
            result.err().map(|error| error.kind()),
            Some(std::io::ErrorKind::UnexpectedEof)
        );
    }

    impl CurrentUserControlClient for RecordingClient {
        fn send(&mut self, command: Command) -> ControlFuture<'_> {
            self.commands.push(command);
            let result = self.results.pop_front().ok_or_else(|| {
                CurrentUserControlError::from(std::io::Error::other("injected result exhausted"))
            });
            Box::pin(std::future::ready(result))
        }
    }

    #[tokio::test]
    async fn injected_confirmation_dispatches_typed_password_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let passphrase = "A-secure-test-passphrase-9!";
        let mut reader = InjectedReader {
            values: VecDeque::from([passphrase, passphrase]),
        };
        let mut client = RecordingClient {
            commands: Vec::new(),
            results: VecDeque::from([CommandResult::ui_password_set(UiAuthView::new(
                true, true, 0,
            ))]),
        };

        change_password(PasswordCommand::Set, &mut reader, &mut client).await?;

        let command = client.commands.first().ok_or("missing recorded command")?;
        assert_eq!(command.operation(), "ui_password_set");
        assert!(!format!("{command:?}").contains(passphrase));
        assert!(
            encode_command(command)?
                .windows(passphrase.len())
                .any(|window| window == passphrase.as_bytes())
        );
        Ok(())
    }

    #[tokio::test]
    async fn mismatched_confirmation_is_rejected_before_control_access() {
        let mut reader = InjectedReader {
            values: VecDeque::from(["A-secure-test-passphrase-9!", "different-passphrase-9!"]),
        };
        let mut client = RecordingClient {
            commands: Vec::new(),
            results: VecDeque::new(),
        };

        let result = change_password(PasswordCommand::Set, &mut reader, &mut client).await;

        assert!(matches!(result, Err(UiCommandError::ConfirmationMismatch)));
        assert!(client.commands.is_empty());
    }

    #[tokio::test]
    async fn revoke_all_dispatches_the_typed_todo_four_command()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut client = RecordingClient {
            commands: Vec::new(),
            results: VecDeque::from([CommandResult::sessions_revoked(UiAuthView::new(
                true, true, 0,
            ))]),
        };

        revoke_all_sessions(&mut client).await?;

        assert_eq!(
            client.commands.first().map(Command::operation),
            Some("session_revoke_all")
        );
        Ok(())
    }
}
