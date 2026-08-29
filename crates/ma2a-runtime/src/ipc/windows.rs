use std::{fs, io};

use interprocess::{
    local_socket::{
        GenericNamespaced, ListenerOptions, ToNsName as _,
        tokio::{Listener, Stream, prelude::*},
    },
    os::windows::{local_socket::ListenerOptionsExt as _, security_descriptor::SecurityDescriptor},
};
use widestring::U16CString;

use super::{
    IpcError, IpcPaths,
    windows_security::{current_process_user_sid, impersonated_client_user_sid},
};

pub(crate) type PlatformListener = Listener;
pub(crate) type PlatformStream = Stream;

pub(crate) fn prepare(paths: &IpcPaths) -> Result<(), IpcError> {
    fs::create_dir_all(paths.runtime_dir())?;
    Ok(())
}

fn name(paths: &IpcPaths) -> Result<interprocess::local_socket::Name<'static>, IpcError> {
    use std::os::windows::ffi::OsStrExt as _;
    let bytes = paths
        .state_dir()
        .as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    let value = format!("ma2a-{}", blake3::hash(&bytes).to_hex());
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

pub(crate) const fn remove_stale_endpoint(_paths: &IpcPaths) -> Result<(), IpcError> {
    Ok(())
}
