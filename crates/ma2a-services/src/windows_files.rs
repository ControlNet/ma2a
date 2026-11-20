#![allow(unsafe_code, reason = "audited Windows file-handle FFI boundary")]
//! What Windows reveals only through a handle: which file it is, where it is, who may use it,
//! and a directory whose protected ACL is in place from the moment it exists.

use std::{
    ffi::OsString,
    fs::File,
    io,
    os::windows::{
        ffi::{OsStrExt as _, OsStringExt as _},
        io::AsRawHandle as _,
    },
    path::{Path, PathBuf},
    ptr,
};

use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW,
            ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
            SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateDirectoryW, FILE_NAME_NORMALIZED,
        GetFileInformationByHandle, GetFinalPathNameByHandleW,
    },
};

/// Full access for the object's owner (`OW`) and SYSTEM only, inherited by everything created
/// inside, and nothing inherited from the parent (`P`).
const PRIVATE: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)";

/// Which file a handle refers to: its volume's serial number and its index on that volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileId(u32, u64);

/// Returns the identity of the file `file` refers to.
///
/// # Errors
/// Returns the operating system's error.
pub fn file_id(file: &File) -> io::Result<FileId> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file`, and
    // `information` is a valid, writable structure of the expected type.
    let read = unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut information) };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }
    let index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(FileId(information.dwVolumeSerialNumber, index))
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
        buffer.resize(length, 0);
    }
}

/// Returns the owner and DACL of the object `file` refers to, in SDDL form. The handle must
/// have been opened with `READ_CONTROL`.
///
/// # Errors
/// Returns the operating system's error.
pub fn owner_and_dacl(file: &File) -> io::Result<String> {
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file`; the
    // descriptor out-pointer is valid and every other out-pointer is optional.
    let status = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            information,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ));
    }
    let mut text = ptr::null_mut();
    let mut length = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] `descriptor` was returned by GetSecurityInfo above
    // and is still allocated; both out-pointers are valid.
    let converted = unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            information,
            &raw mut text,
            &raw mut length,
        )
    };
    let error = io::Error::last_os_error();
    let sddl = if converted == 0 {
        Err(error)
    } else {
        let units = usize::try_from(length).unwrap_or(0);
        // SAFETY: [Category 8 - FFI boundary] on success `text` points at `length` UTF-16
        // units, the terminator included, allocated by the conversion.
        let slice = unsafe { std::slice::from_raw_parts(text, units) };
        let slice = slice.split(|unit| *unit == 0).next().unwrap_or_default();
        Ok(String::from_utf16_lossy(slice))
    };
    // SAFETY: [Category 8 - FFI boundary] `text` was allocated with LocalAlloc by the
    // conversion above, or is null, and is freed exactly once, after its last use.
    let _text = unsafe { LocalFree(text.cast()) };
    // SAFETY: [Category 8 - FFI boundary] `descriptor` was allocated with LocalAlloc by
    // GetSecurityInfo above and is freed exactly once, after its last use.
    let _descriptor = unsafe { LocalFree(descriptor) };
    sddl
}

/// Creates `path`, which must not exist yet, under the [`PRIVATE`] ACL.
///
/// # Errors
/// Returns [`io::ErrorKind::AlreadyExists`] when anything already has that name, or another
/// local error.
pub fn create_protected_dir(path: &Path) -> io::Result<()> {
    let sddl = PRIVATE
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
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(u32::MAX),
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    // SAFETY: [Category 8 - FFI boundary] `wide` is a live NUL-terminated UTF-16 path and
    // `attributes` points at the descriptor converted above, which is still allocated.
    let created = unsafe { CreateDirectoryW(wide.as_ptr(), &raw const attributes) };
    let error = io::Error::last_os_error();
    // SAFETY: [Category 8 - FFI boundary] `descriptor` was allocated by the conversion above
    // with LocalAlloc and is freed exactly once, after its last use.
    let _freed = unsafe { LocalFree(descriptor) };
    if created == 0 { Err(error) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use std::os::windows::fs::OpenOptionsExt as _;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, READ_CONTROL,
    };

    use super::{create_protected_dir, file_id, final_path, owner_and_dacl};

    fn open_directory(path: &std::path::Path) -> std::io::Result<std::fs::File> {
        std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }

    #[test]
    fn a_protected_directory_is_private_from_the_moment_it_exists()
    -> Result<(), Box<dyn std::error::Error>> {
        let user = crate::windows_security::current_process_user_sid()?.to_sddl()?;
        let base = std::env::temp_dir().join(format!("ma2a-acl-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&base);
        std::fs::create_dir(&base)?;
        let path = base.join("private");
        create_protected_dir(&path)?;
        assert!(
            create_protected_dir(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        let (private, inherited) = (open_directory(&path)?, open_directory(&base)?);
        let sddl = owner_and_dacl(&private)?;
        assert!(crate::sddl::is_private(&sddl, &user), "{sddl}");
        // A directory made the ordinary way inherits from the temporary directory.
        let sddl = owner_and_dacl(&inherited)?;
        assert!(!crate::sddl::is_private(&sddl, &user), "{sddl}");
        // Identity and location come from the handle.
        assert_eq!(file_id(&private)?, file_id(&open_directory(&path)?)?);
        assert_ne!(file_id(&private)?, file_id(&inherited)?);
        assert_eq!(final_path(&private)?, std::fs::canonicalize(&path)?);
        drop((private, inherited));
        std::fs::remove_dir_all(&base)?;
        Ok(())
    }
}
