use std::{
    fs, io,
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _},
};

use interprocess::local_socket::{
    GenericFilePath, ListenerOptions, ToFsName as _,
    tokio::{Listener, Stream, prelude::*},
};

use super::{IpcError, IpcPaths};

pub(crate) type PlatformListener = Listener;
pub(crate) type PlatformStream = Stream;

pub(crate) fn prepare(paths: &IpcPaths) -> Result<(), IpcError> {
    let mut builder = fs::DirBuilder::new();
    builder
        .recursive(true)
        .mode(0o700)
        .create(paths.runtime_dir())?;
    let metadata = fs::symlink_metadata(paths.runtime_dir())?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(IpcError::UnauthorizedPeer);
    }
    Ok(())
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
}
