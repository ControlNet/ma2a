#![allow(unsafe_code, reason = "audited Windows ACL FFI boundary")]
//! Who may use a Windows object: its owner and DACL in SDDL form, and whether one SID string is
//! the current user.

use std::{fs::File, io, os::windows::io::AsRawHandle as _, ptr};

use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertStringSidToSidW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION, GetLengthSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID,
    },
};

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
    } else if text.is_null() {
        Ok(String::new())
    } else {
        let units = usize::try_from(length).unwrap_or(0);
        // SAFETY: [Category 8 - FFI boundary] on success `text` is non-null and points at
        // `length` UTF-16 units, the terminator included, allocated by the conversion.
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

/// Returns a predicate telling whether one SDDL SID string is the current process's user.
///
/// The string may be in `S-1-…` form or a two-letter alias. Windows parses it, so an alias
/// stands for whatever account the system itself means by it.
///
/// # Errors
/// Returns the operating system's error when the current user cannot be read.
pub fn current_user_matcher() -> io::Result<impl Fn(&str) -> bool> {
    let user = crate::windows_security::current_process_user_sid()?;
    Ok(move |text: &str| sid_bytes(text).is_ok_and(|bytes| bytes == user.as_bytes()))
}

/// Parses one SDDL SID string, full or an alias, into its binary form.
fn sid_bytes(text: &str) -> io::Result<Vec<u8>> {
    if text.contains('\0') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let wide = text
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] `wide` is a live NUL-terminated UTF-16 string and
    // `sid` is a valid out-pointer.
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &raw mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: [Category 8 - FFI boundary] `sid` is the valid SID the conversion just returned.
    let length = usize::try_from(unsafe { GetLengthSid(sid) }).unwrap_or(0);
    // SAFETY: [Category 8 - FFI boundary] `sid` points at `length` bytes, as GetLengthSid
    // reports for the SID the conversion allocated.
    let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) }.to_vec();
    // SAFETY: [Category 8 - FFI boundary] `sid` was allocated with LocalAlloc by the conversion
    // above and is freed exactly once, after its last use.
    let _freed = unsafe { LocalFree(sid) };
    Ok(bytes)
}
