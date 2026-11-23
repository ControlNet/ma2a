//! Writing an invitation ticket file readable only by the current account.

use ma2a_core::ProtocolError;

#[cfg(unix)]
pub(super) fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
    if path.contains('\0') {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    file.write_all(contents)
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|_| ProtocolError::INTERNAL)
}

#[cfg(windows)]
pub(super) fn write_owner_only(path: &str, contents: &[u8]) -> Result<(), ProtocolError> {
    use std::{fmt::Write as _, io::Write as _, path::Path};

    use ma2a_core::RequestId;

    if path.contains('\0') {
        return Err(ProtocolError::INVALID_INPUT);
    }
    let destination = Path::new(path);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let _name = destination
        .file_name()
        .ok_or(ProtocolError::INVALID_INPUT)?;
    let request = RequestId::random().map_err(|_| ProtocolError::INTERNAL)?;
    let mut dir_name = String::from(".ma2a-invite-stage-");
    for byte in request.as_bytes() {
        let _written = write!(dir_name, "{byte:02x}");
    }
    let staging = parent.join(dir_name);
    ma2a_services::windows_files::create_protected_dir(&staging)
        .map_err(|_| ProtocolError::INVALID_INPUT)?;
    let staged = staging.join("ticket");
    let published = (|| {
        let mut file = ma2a_services::windows_files::create_protected_file(&staged)
            .map_err(|_| ProtocolError::INTERNAL)?;
        file.write_all(contents)
            .and_then(|()| file.write_all(b"\n"))
            .and_then(|()| file.sync_all())
            .map_err(|_| ProtocolError::INTERNAL)?;
        drop(file);
        ma2a_services::windows_files::rename_no_clobber(&staged, destination)
            .map_err(|_| ProtocolError::INVALID_INPUT)
    })();
    if published.is_err() {
        let _removed = std::fs::remove_file(&staged);
    }
    let _removed = std::fs::remove_dir(&staging);
    published
}
