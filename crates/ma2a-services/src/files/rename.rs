//! Atomic no-clobber rename on each supported platform.

use std::path::Path;

use ma2a_core::service::ServiceError;

#[cfg(unix)]
use crate::fs_error;

/// Renames `from` to `to`, failing with `already_exists` instead of replacing `to`.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
pub(crate) fn no_clobber(from: &Path, to: &Path) -> Result<(), ServiceError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    fs_error::map(
        renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(std::io::Error::from),
    )
}

/// Renames `from` to `to`, failing with `already_exists` instead of replacing `to`.
///
/// Other Unix hosts lack a no-clobber rename primitive; a hard link reserves the name
/// atomically for files, and directories are refused rather than raced.
#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
pub(crate) fn no_clobber(from: &Path, to: &Path) -> Result<(), ServiceError> {
    if std::fs::symlink_metadata(from).is_ok_and(|metadata| metadata.is_dir()) {
        return Err(ServiceError::Io);
    }
    fs_error::map(std::fs::hard_link(from, to))?;
    fs_error::map(std::fs::remove_file(from))
}

/// Renames `from` to `to`, failing with `already_exists` instead of replacing `to`.
#[cfg(windows)]
pub(crate) fn no_clobber(from: &Path, to: &Path) -> Result<(), ServiceError> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both buffers are NUL-terminated UTF-16 strings that outlive the call, and flag 0
    // requests a plain move that fails when the destination exists.
    #[expect(
        unsafe_code,
        reason = "Windows exposes no-clobber rename only through MoveFileExW"
    )]
    let moved = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
    if moved == 0 {
        Err(crate::fs_error::service_error(
            &std::io::Error::last_os_error(),
        ))
    } else {
        Ok(())
    }
}
