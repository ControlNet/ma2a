use std::{fs, io};

use interprocess::{
    local_socket::{
        GenericNamespaced, ListenerOptions, ToNsName as _,
        tokio::{Listener, Stream, prelude::*},
    },
    os::windows::{local_socket::ListenerOptionsExt as _, security_descriptor::SecurityDescriptor},
};
use widestring::U16CString;

use super::{IpcError, IpcPaths};

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
    let sddl =
        U16CString::from_str("D:P(A;;GA;;;SY)(A;;GA;;;OW)").map_err(|_| IpcError::InvalidPath)?;
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
    if stream.peer_creds()?.pid().is_some_and(|pid| pid != 0) {
        Ok(())
    } else {
        Err(IpcError::UnauthorizedPeer)
    }
}

pub(crate) const fn remove_stale_endpoint(_paths: &IpcPaths) -> Result<(), IpcError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pipe_dacl_is_current_owner_and_system_only() {
        assert_eq!("D:P(A;;GA;;;SY)(A;;GA;;;OW)", "D:P(A;;GA;;;SY)(A;;GA;;;OW)");
    }
}
