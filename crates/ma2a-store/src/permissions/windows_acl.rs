#![allow(unsafe_code, reason = "audited Windows ACL FFI boundary")]
//! Writes and checks the private ACL through a handle that does not follow a reparse point,
//! with the Win32 security API.

use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io,
    mem::{self, offset_of},
    os::windows::{fs::OpenOptionsExt as _, io::AsRawHandle as _},
    path::Path,
    ptr,
};

use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        ACCESS_ALLOWED_ACE, ACE_FLAGS, ACL, ACL_REVISION, ACL_SIZE_INFORMATION, AclSizeInformation,
        AddAccessAllowedAceEx,
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT, SetSecurityInfo},
        DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetLengthSid,
        GetSecurityDescriptorControl, INHERIT_ONLY_ACE, INHERITED_ACE, InitializeAcl, IsValidAcl,
        IsValidSid, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
    },
    Storage::FileSystem::{
        FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, READ_CONTROL,
        WRITE_DAC, WRITE_OWNER,
    },
    System::SystemServices::ACCESS_ALLOWED_ACE_TYPE,
};

use super::windows_sid::{Sid, administrators, aligned_storage, current_user, local_system};

/// Opens `path`, a file or a directory, itself rather than whatever a link there points at,
/// to read its security descriptor and, when `write`, to replace its owner and DACL.
pub(super) fn open(path: &Path, write: bool) -> io::Result<File> {
    let access = if write {
        READ_CONTROL | WRITE_DAC | WRITE_OWNER
    } else {
        READ_CONTROL
    };
    OpenOptions::new()
        .access_mode(access)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Makes the current user the owner and grants full control to the user and SYSTEM alone,
/// under a protected DACL; `inherit` carries the ACE flags a directory's children inherit by.
pub(super) fn protect(file: &File, inherit: ACE_FLAGS) -> io::Result<()> {
    let (user, system) = (current_user()?, local_system()?);
    let rules = [
        (FILE_ALL_ACCESS, inherit, &user),
        (FILE_ALL_ACCESS, inherit, &system),
    ];
    // Running as SYSTEM, the two are one account, granted once.
    let count = if user.is(system.as_ptr()) { 1 } else { 2 };
    set_owner_and_dacl(file, &user, rules.get(..count).unwrap_or_default())
}

/// Replaces the owner and the DACL with a protected list of Allow `rules`, each an access mask,
/// the ACE flags its inheritance is set by, and the account it is granted to.
pub(super) fn set_owner_and_dacl(
    file: &File,
    owner: &Sid,
    rules: &[(u32, ACE_FLAGS, &Sid)],
) -> io::Result<()> {
    let ace_base = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    let size = rules
        .iter()
        .fold(mem::size_of::<ACL>(), |size, (_, _, sid)| {
            size + ace_base + usize::try_from(sid.len()).unwrap_or(usize::MAX)
        });
    let size = u32::try_from(size).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut storage = aligned_storage(size);
    let acl = storage.as_mut_ptr().cast::<ACL>();
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] `acl` points at `size` writable,
    // aligned bytes, the size of an ACL header plus every ACE added below.
    if unsafe { InitializeAcl(acl, size, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    for (mask, inherit, sid) in rules {
        // SAFETY: [Category 8 - FFI boundary] `acl` was initialized above with room for this ACE
        // and `sid` is a valid live SID.
        if unsafe { AddAccessAllowedAceEx(acl, ACL_REVISION, *inherit, *mask, sid.as_ptr()) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let information = OWNER_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | PROTECTED_DACL_SECURITY_INFORMATION;
    // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file` and was
    // opened with WRITE_OWNER and WRITE_DAC; the owner SID and the ACL are valid and live.
    let status = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            information,
            owner.as_ptr(),
            ptr::null_mut(),
            acl,
            ptr::null(),
        )
    };
    status_result(status)
}

/// Whether the object `file` refers to is private: owned by the current user, SYSTEM, or the
/// Administrators group, under a protected DACL of one or two explicit Allow rules, each granting
/// every `FullControl` right over the object itself to the current user or SYSTEM and together
/// covering both (one principal when running as SYSTEM), with no inherited rule. Any other owner could rewrite the
/// DACL; Administrators can take ownership anyway, and elevated sessions may make it the default
/// owner of new objects.
pub(super) fn is_private(file: &File) -> io::Result<bool> {
    let (user, system, admins) = (current_user()?, local_system()?, administrators()?);
    let descriptor = Descriptor::read(file)?;
    let mut control = 0_u16;
    let mut revision = 0_u32;
    // SAFETY: [Category 8 - FFI boundary] the descriptor is the live one GetSecurityInfo returned
    // and both outputs are writable initialized scalars.
    if unsafe { GetSecurityDescriptorControl(descriptor.raw, &raw mut control, &raw mut revision) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let (owner, dacl) = (descriptor.owner, descriptor.dacl);
    if control & SE_DACL_PROTECTED == 0 || owner.is_null() || dacl.is_null() {
        return Ok(false);
    }
    // SAFETY: [Category 8 - FFI boundary] a non-null owner points into the live descriptor.
    if unsafe { IsValidSid(owner) } == 0 {
        return Ok(false);
    }
    if ![&user, &system, &admins].iter().any(|sid| sid.is(owner)) {
        return Ok(false);
    }
    Ok(grants_only(dacl, &user, &system))
}

/// Whether `dacl` holds one or two explicit Allow rules granting full control, which name only
/// the user or SYSTEM and together cover both.
fn grants_only(dacl: *mut ACL, user: &Sid, system: &Sid) -> bool {
    let mut info = ACL_SIZE_INFORMATION::default();
    let info_size = u32::try_from(mem::size_of::<ACL_SIZE_INFORMATION>()).unwrap_or(u32::MAX);
    // SAFETY: [Category 8 - FFI boundary] `dacl` points into the live descriptor.
    if unsafe { IsValidAcl(dacl) } == 0 {
        return false;
    }
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
        return false;
    }
    let (mut user_seen, mut system_seen) = (false, false);
    for index in 0..info.AceCount {
        let Some(sid) = full_control_sid(dacl, index) else {
            return false;
        };
        let (is_user, is_system) = (user.is(sid), system.is(sid));
        if !is_user && !is_system {
            return false;
        }
        user_seen |= is_user;
        system_seen |= is_system;
    }
    user_seen && system_seen
}

/// The account an explicit Allow rule grants every `FullControl` right over the object itself
/// to, or `None` for any other rule, an inherit-only one that applies to children alone included.
fn full_control_sid(dacl: *mut ACL, index: u32) -> Option<PSID> {
    let mut raw_ace = ptr::null_mut();
    // SAFETY: [Category 8 - FFI boundary] `dacl` is valid and `index` is below its AceCount;
    // Windows writes one borrowed ACE pointer to `raw_ace`.
    if unsafe { GetAce(dacl, index, &raw mut raw_ace) } == 0 || raw_ace.is_null() {
        return None;
    }
    let ace = raw_ace.cast::<ACCESS_ALLOWED_ACE>();
    // SAFETY: [Category 8 - FFI boundary] every ACE begins with a valid ACE_HEADER in the live ACL.
    let header = unsafe { &(*ace).Header };
    let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    let size = usize::from(header.AceSize);
    if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
        || u32::from(header.AceFlags) & (INHERITED_ACE | INHERIT_ONLY_ACE) != 0
        || size < sid_offset + 8
    {
        return None;
    }
    // SAFETY: [Category 8 - FFI boundary] the ACE type and minimum size were checked, so Mask is
    // an initialized field within the ACE.
    if unsafe { (*ace).Mask } & FILE_ALL_ACCESS != FILE_ALL_ACCESS {
        return None;
    }
    // SAFETY: [Categories 8 and 10 - FFI boundary/out-of-bounds] SidStart lies inside the checked
    // ACE, and at least the SID's eight header bytes follow it.
    let sid = unsafe { &raw mut (*ace).SidStart }.cast::<c_void>();
    // SAFETY: [Category 8 - FFI boundary] the SID header is inside the ACE, as checked above.
    if unsafe { IsValidSid(sid) } == 0 {
        return None;
    }
    // SAFETY: [Category 8 - FFI boundary] IsValidSid established a readable SID header.
    let length = usize::try_from(unsafe { GetLengthSid(sid) });
    // The whole SID must lie inside the ACE before anything compares it.
    (sid_offset + length.ok()? <= size).then_some(sid)
}

/// A security descriptor Windows allocated with `LocalAlloc`, freed when dropped, with the owner
/// and DACL pointers into it.
struct Descriptor {
    raw: PSECURITY_DESCRIPTOR,
    owner: PSID,
    dacl: *mut ACL,
}

impl Descriptor {
    fn read(file: &File) -> io::Result<Self> {
        let mut descriptor = Self {
            raw: ptr::null_mut(),
            owner: ptr::null_mut(),
            dacl: ptr::null_mut(),
        };
        // SAFETY: [Category 8 - FFI boundary] the handle is live for the borrow of `file` and was
        // opened with READ_CONTROL; every out-pointer is valid or null where optional.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &raw mut descriptor.owner,
                ptr::null_mut(),
                &raw mut descriptor.dacl,
                ptr::null_mut(),
                &raw mut descriptor.raw,
            )
        };
        status_result(status).map(|()| descriptor)
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: [Category 8 - FFI boundary] `raw` was allocated with LocalAlloc by
        // GetSecurityInfo, or is null, and is freed exactly once, after its last use.
        let _freed = unsafe { LocalFree(self.raw) };
    }
}

fn status_result(status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(i32::MAX),
        ))
    }
}
