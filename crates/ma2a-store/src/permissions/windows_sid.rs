#![allow(unsafe_code, reason = "audited Windows SID FFI boundary")]
//! The accounts a private ACL names: the current user, SYSTEM, and the Administrators group.

use std::{ffi::c_void, io, mem, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::{
        CopySid, CreateWellKnownSid, EqualSid, GetLengthSid, GetTokenInformation, IsValidSid, PSID,
        SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WELL_KNOWN_SID_TYPE,
        WinBuiltinAdministratorsSid, WinLocalSystemSid,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: [Category 8 - FFI boundary] this non-null token handle was returned by
        // OpenProcessToken and this owner closes it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// A valid SID held in storage aligned for Windows to read.
pub(super) struct Sid {
    storage: Vec<usize>,
}

impl Sid {
    pub(super) const fn as_ptr(&self) -> PSID {
        self.storage.as_ptr().cast::<c_void>().cast_mut()
    }

    /// The SID's length in bytes.
    pub(super) fn len(&self) -> u32 {
        // SAFETY: [Category 8 - FFI boundary] the storage holds a SID Windows wrote and checked.
        unsafe { GetLengthSid(self.as_ptr()) }
    }

    /// Whether `other`, a valid SID, names this account.
    pub(super) fn is(&self, other: PSID) -> bool {
        // SAFETY: [Category 8 - FFI boundary] both pointers reference valid live SIDs.
        unsafe { EqualSid(self.as_ptr(), other) != 0 }
    }
}

pub(super) fn current_user() -> io::Result<Sid> {
    // SAFETY: [Category 8 - FFI boundary] GetCurrentProcess takes no pointers and returns the
    // current-process pseudo-handle, which is valid for OpenProcessToken.
    let process = unsafe { GetCurrentProcess() };
    let mut token = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] `token` is writable HANDLE storage and `process` is the
    // current-process pseudo-handle guaranteed by Windows.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut token) } == 0 || token.is_null() {
        return Err(io::Error::last_os_error());
    }
    token_user(&OwnedHandle(token))
}

fn token_user(token: &OwnedHandle) -> io::Result<Sid> {
    let mut required = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] null storage with zero size is the documented token-info
    // size query and `required` is writable initialized storage.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &raw mut required) };
    if required == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut token_info = aligned_storage(required);
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] token_info is aligned and contains
    // at least `required` writable bytes, which Windows initializes on success.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            token_info.as_mut_ptr().cast::<c_void>(),
            required,
            &raw mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: [Category 8 - FFI boundary] a successful TokenUser query initialized TOKEN_USER at
    // the aligned start of token_info.
    let sid = unsafe { (*token_info.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    copy_sid(sid)
}

pub(super) fn local_system() -> io::Result<Sid> {
    well_known(WinLocalSystemSid)
}

pub(super) fn administrators() -> io::Result<Sid> {
    well_known(WinBuiltinAdministratorsSid)
}

fn well_known(kind: WELL_KNOWN_SID_TYPE) -> io::Result<Sid> {
    let mut storage = aligned_storage(SECURITY_MAX_SID_SIZE);
    let mut size = SECURITY_MAX_SID_SIZE;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] storage has at least the
    // documented SECURITY_MAX_SID_SIZE writable bytes and size reports that exact capacity.
    if unsafe {
        CreateWellKnownSid(
            kind,
            ptr::null_mut(),
            storage.as_mut_ptr().cast::<c_void>(),
            &raw mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(Sid { storage })
}

fn copy_sid(sid: PSID) -> io::Result<Sid> {
    if sid.is_null() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    // SAFETY: [Category 8 - FFI boundary] the SID pointer came from initialized TOKEN_USER storage.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    // SAFETY: [Category 8 - FFI boundary] IsValidSid established a readable SID header.
    let length = unsafe { GetLengthSid(sid) };
    let mut storage = aligned_storage(length);
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] destination storage has `length`
    // writable bytes and GetLengthSid established the same readable source extent.
    if unsafe { CopySid(length, storage.as_mut_ptr().cast::<c_void>(), sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Sid { storage })
}

/// Zeroed storage of at least `bytes` bytes, aligned for any Windows security structure.
pub(super) fn aligned_storage(bytes: u32) -> Vec<usize> {
    let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    vec![0_usize; bytes.div_ceil(mem::size_of::<usize>())]
}
