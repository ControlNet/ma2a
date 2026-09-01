use std::{
    fs::File,
    io,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::Path,
};

use rustix::fs::{Mode, OFlags};

use crate::AppError;

pub(super) fn read_owner_only_invitation(path: &Path) -> Result<String, AppError> {
    read_owner_only_invitation_with(path, || {})
}

fn read_owner_only_invitation_with(
    path: &Path,
    before_open: impl FnOnce(),
) -> Result<String, AppError> {
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Err(AppError::Usage(
            "invite file must be an owner-only regular file",
        ));
    }
    before_open();
    let mut file = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| AppError::Usage("invite file must be an owner-only regular file"))?;
    let metadata = file
        .metadata()
        .map_err(|_| AppError::Usage("invite file must be an owner-only regular file"))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.permissions().mode() & 0o7777 != 0o600
    {
        return Err(AppError::Usage(
            "invite file must be an owner-only regular file",
        ));
    }
    let mut invitation = String::new();
    io::Read::read_to_string(&mut io::Read::take(&mut file, 65_537), &mut invitation)
        .map_err(|_| AppError::Usage("invite file is unreadable or oversized"))?;
    if invitation.len() > 65_536 {
        return Err(AppError::Usage("invite file is unreadable or oversized"));
    }
    Ok(invitation)
}

#[cfg(all(test, target_os = "linux"))]
#[path = "space_unix_tests.rs"]
mod tests;
