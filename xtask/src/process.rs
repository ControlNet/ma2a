use std::{path::Path, process::Command};

use crate::error::XtaskError;

pub(crate) fn run(root: &Path, program: &str, arguments: &[&str]) -> Result<(), XtaskError> {
    let status = Command::new(program)
        .args(arguments)
        .current_dir(root)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(XtaskError::Command {
            command: format!("{program} {}", arguments.join(" ")),
            status: status.code(),
        })
    }
}

pub(crate) fn run_web(root: &Path, arguments: &[&str]) -> Result<(), XtaskError> {
    run(&root.join("web"), "bun", arguments)
}
