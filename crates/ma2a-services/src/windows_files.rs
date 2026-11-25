#![allow(unsafe_code, reason = "audited Windows file-handle FFI boundary")]
//! What Windows reveals only through a handle: which file it is and where it is,
//! and a directory whose protected ACL is in place from the moment it exists.

use std::{
    ffi::{OsString, c_void},
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt as _, OsStringExt as _},
        io::{AsRawHandle as _, FromRawHandle as _},
    },
    path::{Path, PathBuf},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW,
        FILE_ATTRIBUTE_NORMAL, FILE_ID_INFO, FILE_NAME_NORMALIZED, FileIdInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
    },
};

/// Which file a handle refers to: its volume's serial number and its 128-bit file identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId(u64, u128);

/// Returns the identity of the file `file` refers to, using 128-bit IDs (safe on `ReFS` and NTFS).
///
/// # Errors
/// Returns the operating system's error, or `Unsupported` when a legacy volume reports an
/// invalid sentinel identifier.
pub fn file_id(file: &File) -> io::Result<FileId> {
    let mut info = FILE_ID_INFO::default();
    let size = u32::try_from(size_of::<FILE_ID_INFO>()).unwrap_or(u32::MAX);
    // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file`, and
    // `info` is valid writable storage of `size` bytes matching `FileIdInfo`.
    let read = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast::<c_void>(),
            size,
        )
    };
    if read != 0 {
        return Ok(FileId(
            info.VolumeSerialNumber,
            u128::from_le_bytes(info.FileId.Identifier),
        ));
    }
    let mut legacy = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file`, and
    // `legacy` is a valid, writable structure of the expected type.
    let read = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut legacy) };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    let index = (u64::from(legacy.nFileIndexHigh) << 32) | u64::from(legacy.nFileIndexLow);
    if index == u64::MAX {
        return Err(io::Error::from(io::ErrorKind::Unsupported));
    }
    Ok(FileId(
        u64::from(legacy.dwVolumeSerialNumber),
        u128::from(index),
    ))
}

/// Returns the path `file` is reachable at now, with every link and junction resolved.
///
/// # Errors
/// Returns the operating system's error.
pub fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0_u16; 512];
    loop {
        let capacity = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file`,
        // and `buffer` is writable for `capacity` UTF-16 units.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                capacity,
                FILE_NAME_NORMALIZED,
            )
        };
        let length = usize::try_from(length).unwrap_or(usize::MAX);
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        // A result longer than the buffer is the size it needs, terminator included.
        if length < buffer.len() {
            buffer.truncate(length);
            return Ok(OsString::from_wide(&buffer).into());
        }
        let grown = length.max(buffer.len().saturating_add(1));
        if grown > 65_536 {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        buffer.resize(grown, 0);
    }
}

/// Opens the directory `path` with `flags` and holds it so that neither it nor a directory above
/// it can be renamed or removed while the handle lives.
///
/// A handle with attribute access alone takes no part in sharing checks, so the directory is
/// opened to list it as well, and without sharing deletion.
pub(crate) fn pin(path: &Path, flags: u32) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
        FILE_SHARE_WRITE, READ_CONTROL,
    };

    std::fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | READ_CONTROL)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | flags)
        .open(path)
}

/// Opens the directory `path` and pins it (see [`pin`]), and returns the handle with the
/// directory's final path.
///
/// Everything later done by that path stays in this very directory, since Windows refuses to
/// move the directory or any ancestor while it is open.
///
/// # Errors
/// Returns the operating system's error, or `NotADirectory` when `path` names something else.
pub fn pin_directory(path: &Path) -> io::Result<(File, PathBuf)> {
    let handle = pin(path, 0)?;
    if !handle.metadata()?.is_dir() {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    let resolved = final_path(&handle)?;
    Ok((handle, resolved))
}

/// Opens the directory `path` itself, never a link or junction there, and holds it so it can be
/// neither renamed nor removed while the handle lives.
///
/// # Errors
/// Returns the operating system's error, or `PermissionDenied` when `path` is a reparse point or
/// not a directory, or when anyone but this account, SYSTEM, and the Administrators group may
/// use it.
pub fn pin_private_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::MetadataExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    };

    let handle = pin(path, FILE_FLAG_OPEN_REPARSE_POINT)?;
    let metadata = handle.metadata()?;
    let linked = metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    let sddl = crate::windows_acl::owner_and_dacl(&handle)?;
    if linked
        || !metadata.is_dir()
        || !crate::sddl::is_private(&sddl, crate::windows_acl::current_user_matcher()?)
    {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(handle)
}

fn with_private_object<T>(
    path: &Path,
    ace_flags: &str,
    work: impl FnOnce(*const u16, &SECURITY_ATTRIBUTES) -> io::Result<T>,
) -> io::Result<T> {
    let mut wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains an interior NUL character",
        ));
    }
    wide.push(0);
    let user = crate::windows_security::current_process_user_sid()?.to_sddl()?;
    let sddl = format!("D:P(A;{ace_flags};FA;;;{user})(A;{ace_flags};FA;;;SY)")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] `sddl` is a live NUL-terminated UTF-16 string and
    // `descriptor` is a valid out-pointer; the size out-pointer is optional.
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &raw mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 {
        return Err(io::Error::last_os_error());
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let result = work(wide.as_ptr(), &attributes);
    // SAFETY: [Category 8 - FFI boundary] `descriptor` was allocated by the conversion above
    // with LocalAlloc and is freed exactly once, after its last use.
    let _freed = unsafe { LocalFree(descriptor) };
    result
}

/// Creates `path`, which must not exist yet, under a protected owner-and-SYSTEM ACL.
///
/// The explicit current-user SID replaces `OW` so elevated and non-elevated tokens of the same
/// account share access.
///
/// # Errors
/// Returns [`io::ErrorKind::AlreadyExists`] when anything already has that name,
/// [`io::ErrorKind::InvalidInput`] for embedded NUL characters, or another local error.
pub fn create_protected_dir(path: &Path) -> io::Result<()> {
    with_private_object(path, "OICI", |wide, attributes| {
        // SAFETY: [Category 8 - FFI boundary] `wide` is NUL-terminated UTF-16 and `attributes`
        // points at the live security descriptor converted for this call.
        if unsafe { CreateDirectoryW(wide, attributes) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

/// Creates a new regular file at `path` under a protected owner-and-SYSTEM ACL.
///
/// # Errors
/// Returns [`io::ErrorKind::AlreadyExists`] when anything already has that name,
/// [`io::ErrorKind::InvalidInput`] for embedded NUL characters, or another local error.
pub fn create_protected_file(path: &Path) -> io::Result<File> {
    with_private_object(path, "", |wide, attributes| {
        // SAFETY: [Category 8 - FFI boundary] `wide` is NUL-terminated UTF-16 and `attributes`
        // points at the live security descriptor converted for this call.
        let handle = unsafe {
            CreateFileW(
                wide,
                GENERIC_WRITE,
                0,
                attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            // SAFETY: [Category 8 - FFI boundary] `handle` is an owned file handle returned by
            // CreateFileW above and is not shared with any other owner.
            Ok(unsafe { File::from_raw_handle(handle) })
        }
    })
}

/// Renames `from` to `to`, failing with [`io::ErrorKind::AlreadyExists`] instead of replacing
/// an existing file or directory.
///
/// # Errors
/// Returns the operating system's error.
pub fn rename_no_clobber(from: &Path, to: &Path) -> io::Result<()> {
    crate::files::rename_windows::no_clobber_io(from, to)
}

#[cfg(test)]
#[path = "windows_files_tests.rs"]
mod tests;
