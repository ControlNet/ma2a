#![allow(unsafe_code, reason = "audited Windows file-security FFI boundary")]

use std::{
    ffi::c_void,
    fs::File,
    mem::{self, offset_of},
    os::windows::io::AsRawHandle as _,
    ptr,
};

use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetKernelObjectSecurity,
        GetLengthSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, INHERITED_ACE, IsValidAcl, IsValidSecurityDescriptor,
        IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    },
    Storage::FileSystem::FILE_ALL_ACCESS,
    System::SystemServices::ACCESS_ALLOWED_ACE_TYPE,
};

#[path = "relay_tls_windows_sid.rs"]
mod sid;

use sid::{builtin_administrators_sid, current_user_sid, local_system_sid};

pub(super) fn validate_private_key_acl(file: &File) -> Result<(), ()> {
    let mut descriptor = file_security_descriptor(file)?;
    let descriptor_ptr = descriptor.as_mut_ptr().cast::<c_void>();
    validate_descriptor(descriptor_ptr)
}

fn file_security_descriptor(file: &File) -> Result<Vec<usize>, ()> {
    let handle = file.as_raw_handle();
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut required = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] `handle` is borrowed from the live file and the null
    // descriptor with zero size is the documented size-query form.
    unsafe {
        GetKernelObjectSecurity(handle, information, ptr::null_mut(), 0, &raw mut required);
    }
    if required == 0 {
        return Err(());
    }
    let storage = aligned_storage(required)?;
    fill_security_descriptor((handle, information), storage, required)
}

fn fill_security_descriptor(
    (handle, information): (HANDLE, u32),
    mut storage: Vec<usize>,
    mut required: u32,
) -> Result<Vec<usize>, ()> {
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] `storage` is aligned and has at
    // least `required` writable bytes; `handle` remains live for this synchronous query.
    if unsafe {
        GetKernelObjectSecurity(
            handle,
            information,
            storage.as_mut_ptr().cast::<c_void>(),
            required,
            &raw mut required,
        )
    } == 0
    {
        return Err(());
    }
    Ok(storage)
}

fn validate_descriptor(descriptor: PSECURITY_DESCRIPTOR) -> Result<(), ()> {
    // SAFETY: [Category 8 - FFI boundary] the descriptor was initialized by a successful
    // GetKernelObjectSecurity call and remains backed by live aligned storage.
    if unsafe { IsValidSecurityDescriptor(descriptor) } == 0 {
        return Err(());
    }
    let mut user = current_user_sid()?;
    let mut system = local_system_sid()?;
    let mut admins = builtin_administrators_sid()?;
    let (user_ptr, system_ptr, admins_ptr) =
        (user.as_mut_ptr(), system.as_mut_ptr(), admins.as_mut_ptr());
    validate_owner(descriptor, [user_ptr, system_ptr, admins_ptr])?;
    let dacl = protected_dacl(descriptor)?;
    validate_dacl(dacl, user_ptr, system_ptr)
}

fn validate_owner(descriptor: PSECURITY_DESCRIPTOR, trusted: [PSID; 3]) -> Result<(), ()> {
    let mut owner = ptr::null_mut();
    let mut defaulted = 0;
    // SAFETY: [Category 8 - FFI boundary] the descriptor is valid and both output locations are
    // initialized writable storage for pointers and flags owned by Windows.
    if unsafe { GetSecurityDescriptorOwner(descriptor, &raw mut owner, &raw mut defaulted) } == 0
        || owner.is_null()
    {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] Windows returned `owner` from a valid live descriptor.
    if unsafe { IsValidSid(owner) } == 0 {
        return Err(());
    }
    let allowed = trusted.into_iter().any(|candidate| {
        // SAFETY: [Category 8 - FFI boundary] `owner` and `candidate` are validated live SIDs.
        unsafe { EqualSid(owner, candidate) != 0 }
    });
    if allowed { Ok(()) } else { Err(()) }
}

fn protected_dacl(descriptor: PSECURITY_DESCRIPTOR) -> Result<*mut ACL, ()> {
    let mut control = 0_u16;
    let mut revision = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] the descriptor is valid and the control outputs are
    // writable initialized scalars.
    if unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
    {
        return Err(());
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] the descriptor is valid and all output locations remain
    // live for this synchronous call.
    if unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &raw mut present,
            &raw mut dacl,
            &raw mut defaulted,
        )
    } == 0
        || present == 0
        || dacl.is_null()
    {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] `dacl` came from the valid live descriptor above.
    if unsafe { IsValidAcl(dacl) } == 0 {
        return Err(());
    }
    Ok(dacl)
}

fn validate_dacl(dacl: *mut ACL, user: PSID, system: PSID) -> Result<(), ()> {
    let mut info = ACL_SIZE_INFORMATION::default();
    let info_size = u32::try_from(mem::size_of::<ACL_SIZE_INFORMATION>()).map_err(|_| ())?;
    // SAFETY: [Category 8 - FFI boundary] `dacl` is a valid live ACL and `info` has the exact
    // layout and writable size required by AclSizeInformation.
    if unsafe {
        GetAclInformation(
            dacl,
            ptr::from_mut(&mut info).cast::<c_void>(),
            info_size,
            AclSizeInformation,
        )
    } == 0
        || !(1..=2).contains(&info.AceCount)
    {
        return Err(());
    }
    // One or two grants that together cover the user and SYSTEM: when the process runs as
    // SYSTEM the two are one principal, which may be recorded once or twice.
    let mut user_seen = false;
    let mut system_seen = false;
    for index in 0..info.AceCount {
        let sid = allowed_full_control_sid(dacl, index)?;
        // SAFETY: [Category 8 - FFI boundary] `sid` and `user` reference validated live SIDs.
        let is_user = unsafe { EqualSid(sid, user) } != 0;
        // SAFETY: [Category 8 - FFI boundary] `sid` and `system` reference validated live SIDs.
        let is_system = unsafe { EqualSid(sid, system) } != 0;
        if !is_user && !is_system {
            return Err(());
        }
        user_seen |= is_user;
        system_seen |= is_system;
    }
    if user_seen && system_seen {
        Ok(())
    } else {
        Err(())
    }
}

fn allowed_full_control_sid(dacl: *mut ACL, index: u32) -> Result<PSID, ()> {
    let mut raw_ace = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] `dacl` is valid and `index` is below its queried AceCount;
    // Windows writes one borrowed ACE pointer to `raw_ace`.
    if unsafe { GetAce(dacl, index, &raw mut raw_ace) } == 0 || raw_ace.is_null() {
        return Err(());
    }
    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: [Category 8 - FFI boundary] every ACE begins with a valid ACE_HEADER in the live ACL.
    let header = unsafe { &(*ace).Header };
    let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
        || header.AceFlags & u8::try_from(INHERITED_ACE).map_err(|_| ())? != 0
        || usize::from(header.AceSize) < sid_offset + 8
    {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] the ACE type and minimum size were checked, so Mask and
    // SidStart are initialized fields within the ACE allocation.
    if unsafe { (*ace).Mask } != FILE_ALL_ACCESS {
        return Err(());
    }
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] SidStart is inside the checked ACE;
    // the pointer is used only after validating the SID header and full encoded length below.
    let sid = unsafe { &raw mut (*ace).SidStart }.cast::<c_void>();
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] the minimum-size check proves the
    // SID revision and sub-authority-count bytes are inside this ACE.
    let count_ptr = unsafe { sid.cast::<u8>().add(1) };
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] `count_ptr` points at the valid
    // sub-authority-count byte inside the live ACE.
    let sub_authority_count = usize::from(unsafe { *count_ptr });
    let encoded_sid_length = 8_usize
        .checked_add(sub_authority_count.checked_mul(4).ok_or(())?)
        .ok_or(())?;
    if sid_offset.checked_add(encoded_sid_length).ok_or(())? > usize::from(header.AceSize) {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] the SID's count-derived full extent was proven to lie
    // inside the live ACE before asking Windows to validate its remaining invariants.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] `sid` was validated and remains inside the live ACE.
    let sid_length = usize::try_from(unsafe { GetLengthSid(sid) }).map_err(|_| ())?;
    if sid_length != encoded_sid_length {
        return Err(());
    }
    Ok(sid)
}

fn aligned_storage(bytes: u32) -> Result<Vec<usize>, ()> {
    let bytes = usize::try_from(bytes).map_err(|_| ())?;
    let word = mem::size_of::<usize>();
    let words = bytes.checked_add(word - 1).ok_or(())? / word;
    Ok(vec![0_usize; words])
}
