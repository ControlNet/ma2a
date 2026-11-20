#![allow(unsafe_code, reason = "audited Windows no-clobber rename FFI boundary")]
//! Windows exposes an atomic no-clobber rename only through `MoveFileExW`.

use std::path::Path;

use ma2a_core::service::ServiceError;

/// Renames `from` to `to`, failing with `already_exists` instead of replacing `to`.
pub(crate) fn no_clobber(from: &Path, to: &Path) -> Result<(), ServiceError> {
    no_clobber_io(from, to).map_err(|error| crate::fs_error::service_error(&error))
}

/// Renames `from` to `to`, failing with [`std::io::ErrorKind::AlreadyExists`] instead of
/// replacing `to`.
pub(crate) fn no_clobber_io(from: &Path, to: &Path) -> std::io::Result<()> {
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
    let moved = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
