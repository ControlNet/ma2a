use std::{fs, io, os::unix::fs::PermissionsExt as _, path::Path};

use interprocess::local_socket::{
    GenericFilePath, ListenerOptions, ToFsName as _,
    tokio::{Listener, Stream, prelude::*},
};

use super::{IpcError, IpcPaths};

pub(crate) type PlatformListener = Listener;
pub(crate) type PlatformStream = Stream;

pub(crate) fn prepare(paths: &IpcPaths) -> Result<(), IpcError> {
    ensure_private_dir(paths.state_dir(), "state directory")?;
    ensure_private_dir(paths.runtime_dir(), "runtime directory")
}

fn ensure_private_dir(path: &Path, target: &'static str) -> Result<(), IpcError> {
    ma2a_store::ensure_private_directory(path, target).map_err(store_dir_error)
}

fn store_dir_error(error: ma2a_store::StoreError) -> IpcError {
    match error {
        ma2a_store::StoreError::InvalidStateDirectory => IpcError::InvalidPath,
        ma2a_store::StoreError::InsecurePermissions { .. } => IpcError::UnauthorizedPeer,
        ma2a_store::StoreError::Io(error) => IpcError::Io(error),
        other => IpcError::Io(io::Error::other(other)),
    }
}

pub(crate) fn bind(paths: &IpcPaths) -> Result<PlatformListener, IpcError> {
    prepare(paths)?;
    let name = paths.socket_path().to_fs_name::<GenericFilePath>()?;
    let listener = ListenerOptions::new()
        .name(name)
        .reclaim_name(false)
        .create_tokio()?;
    fs::set_permissions(paths.socket_path(), fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub(crate) async fn connect(paths: &IpcPaths) -> Result<PlatformStream, IpcError> {
    let name = paths.socket_path().to_fs_name::<GenericFilePath>()?;
    tokio::time::timeout(super::IO_DEADLINE, Stream::connect(name))
        .await
        .map_err(|_| IpcError::Io(io::Error::from(io::ErrorKind::TimedOut)))?
        .map_err(IpcError::from)
}

pub(crate) async fn accept(listener: &PlatformListener) -> Result<PlatformStream, IpcError> {
    Ok(listener.accept().await?)
}

pub(crate) fn authorize(stream: &PlatformStream, _paths: &IpcPaths) -> Result<(), IpcError> {
    let credentials = stream.peer_creds()?;
    authorize_euid(credentials.euid(), rustix::process::geteuid().as_raw())
}

fn authorize_euid(actual: Option<u32>, expected: u32) -> Result<(), IpcError> {
    if actual == Some(expected) {
        Ok(())
    } else {
        Err(IpcError::UnauthorizedPeer)
    }
}

pub(crate) fn remove_stale_endpoint(paths: &IpcPaths) -> Result<(), IpcError> {
    match fs::remove_file(paths.socket_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_peer_with_different_effective_user() {
        // Given
        let expected = 1000;

        // When
        let result = authorize_euid(Some(1001), expected);

        // Then
        assert!(matches!(result, Err(IpcError::UnauthorizedPeer)));
    }

    #[test]
    fn prepare_rejects_shared_or_symlinked_state_directory_before_creating_runtime_dir()
    -> Result<(), Box<dyn std::error::Error>> {
        let base = std::env::temp_dir().join(format!("ma2a-ipc-prepare-{}", std::process::id()));
        let _stale = fs::remove_dir_all(&base);
        let shared = base.join("shared");
        let target = base.join("private");
        let link = base.join("link");
        fs::create_dir_all(&shared)?;
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755))?;
        fs::create_dir(&target)?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700))?;
        std::os::unix::fs::symlink(&target, &link)?;

        let shared_result = IpcPaths::new(&shared)?.prepare();
        let link_result = IpcPaths::new(&link)?.prepare();
        let shared_created = shared.join("run-v1").exists();
        let link_created = target.join("run-v1").exists();
        fs::remove_dir_all(&base)?;

        assert!(
            matches!(shared_result, Err(IpcError::UnauthorizedPeer)),
            "{shared_result:?}"
        );
        assert!(
            matches!(link_result, Err(IpcError::UnauthorizedPeer)),
            "{link_result:?}"
        );
        assert!(!shared_created, "run-v1 was created in a 0755 state dir");
        assert!(
            !link_created,
            "run-v1 was created through a symlinked state dir"
        );
        Ok(())
    }
}
