#![allow(unsafe_code, reason = "audited Windows file-owner FFI boundary")]

use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io::Read as _,
    mem,
    os::windows::{
        fs::{MetadataExt as _, OpenOptionsExt as _},
        io::AsRawHandle as _,
    },
    path::Path,
    ptr,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::{
        EqualSid, GetSecurityDescriptorOwner, GetTokenInformation, IsValidSecurityDescriptor,
        IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    },
    Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, GetKernelObjectSecurity,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::AppError;

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: [Category 8 - FFI boundary] this token handle was returned by OpenProcessToken
        // and this owner closes it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn read_owner_only_invitation(path: &Path) -> Result<String, AppError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| AppError::Usage("invite file must be an owner-only regular file"))?;
    let metadata = file
        .metadata()
        .map_err(|_| AppError::Usage("invite file must be an owner-only regular file"))?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !owned_by_current_user(&file)
    {
        return Err(AppError::Usage(
            "invite file must be an owner-only regular file",
        ));
    }
    let mut invitation = String::new();
    file.take(65_537)
        .read_to_string(&mut invitation)
        .map_err(|_| AppError::Usage("invite file is unreadable or oversized"))?;
    if invitation.len() > 65_536 {
        return Err(AppError::Usage("invite file is unreadable or oversized"));
    }
    Ok(invitation)
}

fn owned_by_current_user(file: &File) -> bool {
    file_owner_matches_current_user(file).is_ok()
}

fn file_owner_matches_current_user(file: &File) -> Result<(), ()> {
    let descriptor = file_security_descriptor(file)?;
    let descriptor = descriptor.as_ptr().cast_mut().cast::<c_void>();
    // SAFETY: [Category 8 - FFI boundary] a successful GetKernelObjectSecurity call initialized
    // the descriptor in live aligned storage.
    if unsafe { IsValidSecurityDescriptor(descriptor) } == 0 {
        return Err(());
    }
    let mut owner = ptr::null_mut();
    let mut defaulted = 0;
    // SAFETY: [Category 8 - FFI boundary] the descriptor is valid and both output locations are
    // initialized writable storage.
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) } == 0
        || owner.is_null()
        || unsafe { IsValidSid(owner) } == 0
    {
        return Err(());
    }
    let user = current_user_token_info()?;
    // SAFETY: [Category 8 - FFI boundary] the successful TokenUser query initialized TOKEN_USER
    // at the aligned start of the live token buffer.
    let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    if user_sid.is_null() || unsafe { IsValidSid(user_sid) } == 0 {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] both pointers reference validated SIDs backed by live
    // descriptor and token storage for this comparison.
    if unsafe { EqualSid(owner, user_sid) } == 0 {
        return Err(());
    }
    Ok(())
}

fn file_security_descriptor(file: &File) -> Result<Vec<usize>, ()> {
    let handle = file.as_raw_handle();
    let mut required = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] handle is borrowed from the live file and null storage
    // with zero size is the documented size-query form.
    unsafe {
        GetKernelObjectSecurity(
            handle,
            OWNER_SECURITY_INFORMATION,
            ptr::null_mut(),
            0,
            &mut required,
        );
    }
    let mut storage = aligned_storage(required)?;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] storage is aligned and contains
    // at least required writable bytes; the file handle remains live during the call.
    if unsafe {
        GetKernelObjectSecurity(
            handle,
            OWNER_SECURITY_INFORMATION,
            storage.as_mut_ptr().cast::<c_void>(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(());
    }
    Ok(storage)
}

fn current_user_token_info() -> Result<Vec<usize>, ()> {
    // SAFETY: [Category 8 - FFI boundary] GetCurrentProcess takes no pointers and returns a valid
    // current-process pseudo-handle for OpenProcessToken.
    let process = unsafe { GetCurrentProcess() };
    let mut token = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] token is writable HANDLE storage and process is the
    // current-process pseudo-handle guaranteed by Windows.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
        return Err(());
    }
    let token = OwnedHandle(token);
    let mut required = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] null storage with zero size is the documented token-info
    // size query and required is writable initialized storage.
    unsafe { GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut required) };
    let mut storage = aligned_storage(required)?;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] storage is aligned and contains
    // at least required writable bytes, which Windows initializes on success.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            storage.as_mut_ptr().cast::<c_void>(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(());
    }
    Ok(storage)
}

fn aligned_storage(bytes: u32) -> Result<Vec<usize>, ()> {
    let bytes = usize::try_from(bytes).map_err(|_| ())?;
    if bytes == 0 {
        return Err(());
    }
    let word = mem::size_of::<usize>();
    let words = bytes.checked_add(word - 1).ok_or(())? / word;
    Ok(vec![0_usize; words])
}
