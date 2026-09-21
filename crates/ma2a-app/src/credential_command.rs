use std::{io::Write as _, path::Path};

use ma2a_runtime::ipc::IpcPaths;

use crate::{
    AppError,
    commands::{
        control::RuntimeControlClient,
        ui::{self, InheritedStdinPasswordReader, TerminalPasswordReader},
    },
};

pub(crate) async fn run_password(state_dir: &Path) -> Result<(), AppError> {
    let paths = IpcPaths::new(state_dir)?;
    let mut client = RuntimeControlClient::new(state_dir.to_path_buf(), paths);
    if std::env::var_os("MA2A_PASSWORD_STDIN").is_some_and(|value| value == "1") {
        let mut reader =
            InheritedStdinPasswordReader::new(std::io::BufReader::new(std::io::stdin()));
        ui::change_password(&mut reader, &mut client)
            .await
            .map_err(AppError::Command)?;
    } else {
        let mut reader = TerminalPasswordReader;
        ui::change_password(&mut reader, &mut client)
            .await
            .map_err(AppError::Command)?;
    }
    writeln!(std::io::stdout().lock(), "Web password updated").map_err(AppError::Io)
}

pub(crate) async fn run_revoke_all(state_dir: &Path) -> Result<(), AppError> {
    let paths = IpcPaths::new(state_dir)?;
    let mut client = RuntimeControlClient::new(state_dir.to_path_buf(), paths);
    ui::revoke_all_sessions(&mut client)
        .await
        .map_err(AppError::Command)?;
    writeln!(std::io::stdout().lock(), "All Web sessions revoked").map_err(AppError::Io)
}
