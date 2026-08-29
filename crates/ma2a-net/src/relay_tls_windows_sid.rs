#![allow(unsafe_code, reason = "audited Windows SID FFI boundary")]

use std::{ffi::c_void, mem, ptr};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::{
        CopySid, CreateWellKnownSid, GetLengthSid, GetTokenInformation, IsValidSid, PSID,
        SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER, TokenUser, WinLocalSystemSid,
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

pub(super) struct AlignedSid {
    storage: Vec<usize>,
}

impl AlignedSid {
    pub(super) fn as_ptr(&self) -> PSID {
        self.storage.as_ptr().cast_mut().cast::<c_void>()
    }
}

pub(super) fn current_user_sid() -> Result<AlignedSid, ()> {
    // SAFETY: [Category 8 - FFI boundary] GetCurrentProcess takes no pointers and returns the
    // current-process pseudo-handle, which is valid for OpenProcessToken.
    let process = unsafe { GetCurrentProcess() };
    let mut token = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] `token` is writable HANDLE storage and `process` is the
    // current-process pseudo-handle guaranteed by Windows.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
        return Err(());
    }
    token_user_sid(OwnedHandle(token))
}

fn token_user_sid(token: OwnedHandle) -> Result<AlignedSid, ()> {
    let mut required = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] null storage with zero size is the documented token-info
    // size query and `required` is writable initialized storage.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required) };
    if required == 0 {
        return Err(());
    }
    let mut token_info = aligned_storage(required)?;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] token_info is aligned and contains
    // at least `required` writable bytes, which Windows initializes on success.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            token_info.as_mut_ptr().cast::<c_void>(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] a successful TokenUser query initialized TOKEN_USER at
    // the aligned start of token_info.
    let sid = unsafe { (*token_info.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    copy_sid(sid)
}

pub(super) fn local_system_sid() -> Result<AlignedSid, ()> {
    let mut sid = aligned_storage(SECURITY_MAX_SID_SIZE)?;
    let mut size = SECURITY_MAX_SID_SIZE;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] sid has at least the documented
    // SECURITY_MAX_SID_SIZE writable bytes and size reports that exact capacity.
    if unsafe {
        CreateWellKnownSid(
            WinLocalSystemSid,
            ptr::null_mut(),
            sid.as_mut_ptr().cast::<c_void>(),
            &mut size,
        )
    } == 0
    {
        return Err(());
    }
    Ok(AlignedSid { storage: sid })
}

fn copy_sid(sid: PSID) -> Result<AlignedSid, ()> {
    if sid.is_null() {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] the SID pointer came from initialized TOKEN_USER storage.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] IsValidSid established a readable SID header.
    let length = unsafe { GetLengthSid(sid) };
    let mut storage = aligned_storage(length)?;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] destination storage has `length`
    // writable bytes and GetLengthSid established the same readable source extent.
    if unsafe { CopySid(length, storage.as_mut_ptr().cast::<c_void>(), sid) } == 0 {
        return Err(());
    }
    Ok(AlignedSid { storage })
}

fn aligned_storage(bytes: u32) -> Result<Vec<usize>, ()> {
    let bytes = usize::try_from(bytes).map_err(|_| ())?;
    let word = mem::size_of::<usize>();
    let words = bytes.checked_add(word - 1).ok_or(())? / word;
    Ok(vec![0_usize; words])
}
