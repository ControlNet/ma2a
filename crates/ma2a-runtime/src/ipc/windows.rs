use std::{io, path::Path};

use interprocess::{
    local_socket::{
        GenericNamespaced, ListenerOptions, ToNsName as _,
        tokio::{Listener, Stream, prelude::*},
    },
    os::windows::{local_socket::ListenerOptionsExt as _, security_descriptor::SecurityDescriptor},
};
use widestring::U16CString;

use ma2a_services::windows_security::{current_process_user_sid, impersonated_client_user_sid};

use super::{IpcError, IpcPaths};

pub(crate) type PlatformListener = Listener;
pub(crate) type PlatformStream = Stream;

pub(crate) fn prepare(paths: &IpcPaths) -> Result<(), IpcError> {
    ensure_private_dir(paths.state_dir(), "state directory")?;
    ensure_private_dir(paths.runtime_dir(), "runtime directory")
}

/// Makes sure `path` is a private directory, creating every missing level so that each one
/// carries its protected ACL from the moment it exists. A path the Store would refuse is refused
/// before anything is created.
///
/// A level that appears meanwhile is a lost race, not an error: whatever holds the name is then
/// checked like any existing directory. Every other creation error is returned as it is.
fn ensure_private_dir(path: &Path, target: &'static str) -> Result<(), IpcError> {
    if !ma2a_store::is_local_absolute(path) {
        return Err(IpcError::InvalidPath);
    }
    let missing = path
        .ancestors()
        .take_while(|level| !level.exists())
        .collect::<Vec<_>>();
    let mut created_leaf = false;
    for level in missing.iter().rev() {
        match ma2a_services::windows_files::create_protected_dir(level) {
            Ok(()) => created_leaf = *level == path,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => created_leaf = false,
            Err(error) => return Err(IpcError::Io(error)),
        }
    }
    if created_leaf {
        return ma2a_store::protect_private_directory(path, target).map_err(store_dir_error);
    }
    ma2a_store::ensure_private_directory(path, target).map_err(store_dir_error)
}

fn store_dir_error(error: ma2a_store::StoreError) -> IpcError {
    match error {
        ma2a_store::StoreError::InvalidStateDirectory => IpcError::InvalidPath,
        ma2a_store::StoreError::InsecurePermissions { .. }
        | ma2a_store::StoreError::WindowsAcl { .. } => IpcError::UnauthorizedPeer,
        other => IpcError::Io(io::Error::other(other)),
    }
}

/// Identifies a directory as the file system resolves it, so every spelling of one directory
/// (case, separators, a trailing separator, a link to it) gets the same identity, as its lock
/// file does. A directory that does not exist yet keeps its spelling.
///
/// Every pipe derived from a state directory is named from this identity, so a daemon started
/// under another spelling still reaches the pipes an earlier one named.
pub(crate) fn directory_identity(path: &Path) -> blake3::Hash {
    use std::os::windows::ffi::OsStrExt as _;
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let bytes = resolved
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    blake3::hash(&bytes)
}

/// Names the daemon's pipe after the identity of its state directory.
fn name(paths: &IpcPaths) -> Result<interprocess::local_socket::Name<'static>, IpcError> {
    let value = format!("ma2a-{}", directory_identity(paths.state_dir()).to_hex());
    Ok(value.to_ns_name::<GenericNamespaced>()?.into_owned())
}

pub(crate) fn bind(paths: &IpcPaths) -> Result<PlatformListener, IpcError> {
    prepare(paths)?;
    let current_user = current_process_user_sid()?;
    let sddl = U16CString::from_str(current_user.private_pipe_sddl()?)
        .map_err(|_| IpcError::InvalidPath)?;
    let descriptor = SecurityDescriptor::deserialize(&sddl)?;
    Ok(ListenerOptions::new()
        .name(name(paths)?)
        .security_descriptor(descriptor)
        .create_tokio()?)
}

pub(crate) async fn connect(paths: &IpcPaths) -> Result<PlatformStream, IpcError> {
    tokio::time::timeout(super::IO_DEADLINE, Stream::connect(name(paths)?))
        .await
        .map_err(|_| IpcError::Io(io::Error::from(io::ErrorKind::TimedOut)))?
        .map_err(IpcError::from)
}

pub(crate) async fn accept(listener: &PlatformListener) -> Result<PlatformStream, IpcError> {
    Ok(listener.accept().await?)
}

pub(crate) fn authorize(stream: &PlatformStream, _paths: &IpcPaths) -> Result<(), IpcError> {
    let daemon_user = current_process_user_sid()?;
    let caller_user = impersonated_client_user_sid(stream)?;
    if caller_user == daemon_user {
        Ok(())
    } else {
        Err(IpcError::UnauthorizedPeer)
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "Unix counterpart returns IpcError when unlinking the socket"
)]
pub(crate) const fn remove_stale_endpoint(_paths: &IpcPaths) -> Result<(), IpcError> {
    Ok(())
}

/// Resolves once the connected named-pipe peer process exits, even when unread bytes remain
/// buffered in the pipe or internal read buffer.
pub(crate) async fn peer_disconnected(stream: &PlatformStream) {
    super::pipe_windows::wait_peer_disconnect(stream).await;
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_levels_of_a_state_directory_are_created_private()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("ma2a-cold-start-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base)?;
        let state = base.join("one").join("two").join("state");
        super::ensure_private_dir(&state, "state directory")?;
        // Every level made here is private, not only the last one, and a second start accepts it.
        for level in [
            base.join("one"),
            base.join("one").join("two"),
            state.clone(),
        ] {
            ma2a_store::ensure_private_directory(&level, "created level")?;
        }
        super::ensure_private_dir(&state, "state directory")?;
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn a_state_directory_off_a_drive_letter_is_refused_before_anything_is_created() {
        for path in [
            r"\\?\Volume{00000000-0000-0000-0000-000000000000}\state",
            r"\\?\GLOBALROOT\Device\Mup\server\share\state",
            r"\\server\share\state",
        ] {
            let result = super::ensure_private_dir(std::path::Path::new(path), "state directory");
            assert!(
                matches!(result, Err(super::IpcError::InvalidPath)),
                "{path}: {result:?}"
            );
        }
    }

    #[test]
    fn every_spelling_of_one_state_directory_names_one_pipe()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("ma2a-pipe-name-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        let state = base.join("State");
        std::fs::create_dir_all(&state)?;
        let spelled = state.to_string_lossy().into_owned();
        let named = |path: String| super::name(&super::IpcPaths::new(path)?);

        let name = named(spelled.clone())?;
        assert_eq!(named(spelled.to_lowercase())?, name);
        assert_eq!(named(format!("{spelled}\\"))?, name);
        assert_eq!(named(spelled.replace('\\', "/"))?, name);
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn peer_disconnected_fires_with_unread_bytes_buffered()
    -> Result<(), Box<dyn std::error::Error>> {
        use tokio::io::AsyncBufReadExt as _;

        let base = std::env::temp_dir().join(format!("ma2a-ipc-pipe-hup-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base)?;
        let state = base.join("state");
        let paths = super::IpcPaths::new(&state)?;
        let listener = super::bind(&paths)?;

        let pipe_short = format!("ma2a-{}", super::directory_identity(&state).to_hex());
        let script = format!(
            "try {{ \
                $p = [System.IO.Pipes.NamedPipeClientStream]::new('.', '{pipe_short}', [System.IO.Pipes.PipeDirection]::InOut, [System.IO.Pipes.PipeOptions]::Asynchronous); \
                $p.Connect(5000); \
                $b = [byte[]]::new(8192); \
                $null = $p.WriteAsync($b, 0, $b.Length).Wait(2000); \
                [Console]::Out.WriteLine('ready'); \
                [Console]::Out.Flush(); \
                $null = [Console]::In.ReadLine(); \
            }} catch {{ \
                [Console]::Error.WriteLine($_.Exception.ToString()); \
                exit 1; \
            }}"
        );
        let mut child = tokio::process::Command::new("pwsh")
            .args(["-NoProfile", "-Command", &script])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let _stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("missing child stdout")?;
        let stderr = child.stderr.take().ok_or("missing child stderr")?;
        let mut lines = tokio::io::BufReader::new(stdout).lines();

        let accept_and_ready = async {
            let server = super::accept(&listener).await?;
            let line = lines.next_line().await?;
            Ok::<_, super::IpcError>((server, line))
        };
        let (server, line) = tokio::select! {
            res = tokio::time::timeout(std::time::Duration::from_secs(15), accept_and_ready) => res??,
            status = child.wait() => {
                use tokio::io::AsyncReadExt as _;
                let mut err = String::new();
                let mut err_reader = stderr;
                let _read = err_reader.read_to_string(&mut err).await;
                return Err(format!("pwsh exited early ({status:?}): {err}").into());
            }
        };
        assert_eq!(line.as_deref(), Some("ready"));

        let still_open = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            super::peer_disconnected(&server),
        )
        .await;
        assert!(
            still_open.is_err(),
            "live peer process falsely reported disconnected"
        );

        child.kill().await?;
        let closed = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            super::peer_disconnected(&server),
        )
        .await;
        drop(server);
        drop(listener);
        std::fs::remove_dir_all(&base)?;
        assert!(closed.is_ok(), "terminated peer process was not detected");
        Ok(())
    }
}
