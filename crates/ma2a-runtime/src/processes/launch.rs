//! How this Runtime starts supervisors, in owner-private directories.

use std::{ffi::OsString, io, path::Path, path::PathBuf};

use ma2a_core::service::{ProcessStart, ServiceError};
use ma2a_services::supervisor::{Channel, Credentials, Launch, launch};

use super::ProcessManager;

/// Longest Unix socket path every supported platform accepts (macOS allows 104 bytes).
#[cfg(unix)]
const MAX_SOCKET_PATH: usize = 103;

#[derive(Clone, Debug)]
enum Launcher {
    /// A separate executable that reads the launch record from stdin and supervises.
    Executable(PathBuf, Vec<OsString>),
    /// A supervisor task inside this process, for tests of everything around it.
    #[cfg(test)]
    InProcess,
}

/// How a Runtime starts Process supervisors.
///
/// A Runtime started without it follows, reads, and manages the Processes it already hosts,
/// but refuses to start new ones.
#[derive(Clone, Debug)]
pub struct ProcessHosting(Launcher);

impl ProcessHosting {
    /// Starts each supervisor as `program args…`.
    ///
    /// The program must read one launch record from stdin, detach from this session, and
    /// run `ma2a_services::supervisor::supervise` with it; the `ma2a` executable does this
    /// in its hidden supervisor mode.
    pub const fn executable(program: PathBuf, args: Vec<OsString>) -> Self {
        Self(Launcher::Executable(program, args))
    }

    #[cfg(test)]
    pub(crate) const fn in_process() -> Self {
        Self(Launcher::InProcess)
    }
}

/// Creates one directory readable only by this user.
fn private_dir(path: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        builder.mode(0o700);
    }
    builder.create(path)
}

impl ProcessManager {
    /// Starts the supervisor of one recorded Process and waits for its ownership proof.
    pub(super) async fn launch(
        &self,
        credentials: Credentials,
        start: ProcessStart,
    ) -> Result<Channel, ServiceError> {
        let hosting = self.0.hosting.as_ref().ok_or(ServiceError::Unavailable)?;
        let id = credentials.id();
        let address = self.address(id);
        #[cfg(unix)]
        if address.as_str().len() > MAX_SOCKET_PATH {
            return Err(ServiceError::Unavailable);
        }
        let directory = self.directory(id);
        for path in [&self.0.processes_dir, &self.0.sockets_dir] {
            private_dir(path).map_err(|_| ServiceError::Unavailable)?;
        }
        let record = Launch {
            credentials,
            directory,
            address,
            start,
        };
        match &hosting.0 {
            Launcher::Executable(program, args) => {
                let (mut child, channel) = launch::spawn((program, args), &record).await?;
                // The supervisor outlives this Runtime; while both live, it is reaped here.
                // Shutdown stops only the reaping (the child is not killed on drop), so it never
                // waits for a supervisor.
                let shutdown = self.0.shutdown.clone();
                self.0.tasks.spawn(async move {
                    tokio::select! {
                        _reaped = child.wait() => {}
                        () = shutdown.cancelled() => {}
                    }
                });
                Ok(channel)
            }
            #[cfg(test)]
            Launcher::InProcess => {
                let address = record.address.clone();
                tokio::spawn(ma2a_services::supervisor::supervise(record));
                launch::ready(&address, &credentials).await
            }
        }
    }

    /// Removes the directory of a Process whose output is no longer retained.
    pub(super) async fn remove_directory(&self, id: ma2a_core::service::ProcessId) {
        let directory = self.directory(id);
        let _removed =
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(directory)).await;
    }
}
