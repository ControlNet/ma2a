#![allow(unsafe_code, reason = "audited Windows file-owner FFI boundary")]

use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io::Read as _,
    mem::{self, offset_of},
    os::windows::{
        fs::{MetadataExt as _, OpenOptionsExt as _},
        io::AsRawHandle as _,
    },
    path::Path,
    ptr,
};

use windows_sys::Win32::{
    Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetKernelObjectSecurity,
        GetLengthSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, INHERITED_ACE, IsValidAcl, IsValidSecurityDescriptor,
        IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    },
    Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT,
    },
    System::SystemServices::ACCESS_ALLOWED_ACE_TYPE,
};

#[path = "space_windows_sid.rs"]
mod sid;

use super::space_windows_policy::{AccessRule, Principal, is_owner_only};
use crate::AppError;
use sid::{current_user_sid, local_system_sid};

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
        || !has_owner_only_acl(&file)
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

fn has_owner_only_acl(file: &File) -> bool {
    file_owner_only_policy(file).is_ok()
}

fn file_owner_only_policy(file: &File) -> Result<(), ()> {
    let descriptor = file_security_descriptor(file)?;
    let descriptor = descriptor.as_ptr().cast_mut().cast::<c_void>();
    // SAFETY: [Category 8 - FFI boundary] a successful GetKernelObjectSecurity call initialized
    // the descriptor in live aligned storage.
    if unsafe { IsValidSecurityDescriptor(descriptor) } == 0 {
        return Err(());
    }
    let user = current_user_sid()?;
    let system = local_system_sid()?;
    let owner = descriptor_owner(descriptor, user.as_ptr())?;
    let (protected, dacl) = descriptor_dacl(descriptor)?;
    let rules = dacl_rules(dacl, user.as_ptr(), system.as_ptr())?;
    if !is_owner_only(owner, protected, &rules) {
        return Err(());
    }
    Ok(())
}

fn descriptor_owner(descriptor: PSECURITY_DESCRIPTOR, user: PSID) -> Result<Principal, ()> {
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
    // SAFETY: [Category 8 - FFI boundary] owner and user reference validated SIDs backed by live
    // descriptor and token storage for this comparison.
    Ok(if unsafe { EqualSid(owner, user) } != 0 {
        Principal::CurrentUser
    } else {
        Principal::Other
    })
}

fn descriptor_dacl(descriptor: PSECURITY_DESCRIPTOR) -> Result<(bool, *mut ACL), ()> {
    let mut control = 0_u16;
    let mut revision = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] the descriptor is valid and both output scalars are live
    // writable storage for this synchronous query.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(());
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] the descriptor is valid and all output locations remain
    // live for this synchronous query.
    if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
        == 0
        || present == 0
        || dacl.is_null()
        || unsafe { IsValidAcl(dacl) } == 0
    {
        return Err(());
    }
    Ok((control & SE_DACL_PROTECTED != 0, dacl))
}

fn dacl_rules(dacl: *mut ACL, user: PSID, system: PSID) -> Result<Vec<AccessRule>, ()> {
    let mut info = ACL_SIZE_INFORMATION::default();
    let info_size = u32::try_from(mem::size_of::<ACL_SIZE_INFORMATION>()).map_err(|_| ())?;
    // SAFETY: [Category 8 - FFI boundary] dacl is a validated live ACL and info has the exact
    // writable layout required by AclSizeInformation.
    if unsafe {
        GetAclInformation(
            dacl,
            ptr::from_mut(&mut info).cast::<c_void>(),
            info_size,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(());
    }
    (0..info.AceCount)
        .map(|index| access_rule(dacl, index, user, system))
        .collect()
}

fn access_rule(dacl: *mut ACL, index: u32, user: PSID, system: PSID) -> Result<AccessRule, ()> {
    let mut raw_ace = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] index is below the queried AceCount and Windows writes
    // one borrowed ACE pointer backed by the live ACL.
    if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 || raw_ace.is_null() {
        return Err(());
    }
    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: [Category 8 - FFI boundary] every ACE begins with a valid ACE_HEADER inside the live
    // ACL returned by Windows.
    let header = unsafe { &(*ace).Header };
    let inherited = header.AceFlags & u8::try_from(INHERITED_ACE).map_err(|_| ())? != 0;
    if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE {
        return Ok(AccessRule {
            principal: Principal::Other,
            allowed: false,
            inherited,
            full_control: false,
        });
    }
    let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    if usize::from(header.AceSize) < sid_offset + 8 {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] the allowed ACE type and minimum size establish that
    // Mask is an initialized field inside the live ACE.
    let full_control = unsafe { (*ace).Mask } == FILE_ALL_ACCESS;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] SidStart lies inside the checked
    // ACE and its full count-derived extent is validated before Windows consumes it.
    let sid = unsafe { ptr::addr_of_mut!((*ace).SidStart).cast::<c_void>() };
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] the minimum ACE size proves the SID
    // revision and sub-authority count bytes are readable.
    let sub_authority_count = usize::from(unsafe { *sid.cast::<u8>().add(1) });
    let sid_length = 8_usize
        .checked_add(sub_authority_count.checked_mul(4).ok_or(())?)
        .ok_or(())?;
    if sid_offset.checked_add(sid_length).ok_or(())? > usize::from(header.AceSize) {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] the count-derived SID extent lies inside the live ACE.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] sid is validated and remains backed by the live ACE.
    if usize::try_from(unsafe { GetLengthSid(sid) }).map_err(|_| ())? != sid_length {
        return Err(());
    }
    // SAFETY: [Category 8 - FFI boundary] sid and user reference validated live SIDs.
    let principal = if unsafe { EqualSid(sid, user) } != 0 {
        Principal::CurrentUser
    // SAFETY: [Category 8 - FFI boundary] sid and system reference validated live SIDs.
    } else if unsafe { EqualSid(sid, system) } != 0 {
        Principal::System
    } else {
        Principal::Other
    };
    Ok(AccessRule {
        principal,
        allowed: true,
        inherited,
        full_control,
    })
}

fn file_security_descriptor(file: &File) -> Result<Vec<usize>, ()> {
    let handle = file.as_raw_handle();
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut required = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] handle is borrowed from the live file and null storage
    // with zero size is the documented size-query form.
    unsafe {
        GetKernelObjectSecurity(handle, information, ptr::null_mut(), 0, &mut required);
    }
    let mut storage = aligned_storage(required)?;
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] storage is aligned and contains
    // at least required writable bytes; the file handle remains live during the call.
    if unsafe {
        GetKernelObjectSecurity(
            handle,
            information,
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
