//! Child process construction, whole-group termination, and exit classification.

use std::{path::PathBuf, process::ExitStatus};

use ma2a_core::service::{ProcessSpec, ServiceError, Termination};

/// Returns the absolute working directory for a spec: its `cwd`, else the home directory.
pub(crate) fn working_directory(spec: &ProcessSpec) -> Result<PathBuf, ServiceError> {
    let directory = match spec.cwd() {
        Some(cwd) => PathBuf::from(cwd),
        None => std::env::home_dir().ok_or(ServiceError::SpawnFailed)?,
    };
    if directory.is_absolute() {
        Ok(directory)
    } else {
        Err(ServiceError::InvalidInput)
    }
}

/// Builds a command from typed argv; nothing is ever interpreted by a shell.
pub(crate) fn command(
    program: &str,
    spec: &ProcessSpec,
) -> Result<tokio::process::Command, ServiceError> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(spec.args())
        .current_dir(working_directory(spec)?)
        .envs(spec.env().iter().map(|(name, value)| (name, value)))
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    Ok(command)
}

/// One spawned child and the process-group ID captured before it could be reaped.
#[derive(Debug)]
pub(crate) struct Spawned {
    pub(crate) child: tokio::process::Child,
    #[cfg(unix)]
    group: Option<u32>,
}

impl Spawned {
    /// Records the child's process ID, which is its process-group ID on Unix.
    #[cfg_attr(
        not(unix),
        expect(
            clippy::missing_const_for_fn,
            reason = "only the Unix build captures a group ID at construction"
        )
    )]
    pub(crate) fn new(child: tokio::process::Child) -> Self {
        Self {
            #[cfg(unix)]
            group: child.id(),
            child,
        }
    }

    /// Kills the child and, on Unix, every process left in its process group.
    ///
    /// The group ID is captured at spawn because the child's own ID is unavailable once it
    /// was reaped. Windows kills only the direct child; descendants that detach are a
    /// documented limitation.
    pub(crate) fn kill_all(&mut self) {
        #[cfg(unix)]
        if let Some(group) = self
            .group
            .and_then(|id| i32::try_from(id).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _signalled =
                rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
        }
        let _killed = self.child.start_kill();
    }
}

/// Classifies an exit status without exposing platform detail beyond code or signal.
pub(crate) fn termination(status: ExitStatus) -> Termination {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status
            .signal()
            .and_then(|signal| u32::try_from(signal).ok())
        {
            return Termination::Signaled(signal);
        }
    }
    status.code().map_or(Termination::Unknown, |code| {
        Termination::Exited(u32::from_ne_bytes(code.to_ne_bytes()))
    })
}
