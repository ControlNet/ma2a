#![allow(
    unsafe_code,
    reason = "audited Windows security-descriptor FFI boundary"
)]
//! A directory whose protected ACL is in place from the moment it exists.

use std::{io, os::windows::ffi::OsStrExt as _, path::Path, ptr};

use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::CreateDirectoryW,
};

/// Full access for the object's owner (`OW`) and SYSTEM only, inherited by everything created
/// inside, and nothing inherited from the parent (`P`).
const PRIVATE: &str = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)";

/// Creates `path`, which must not exist yet, under the [`PRIVATE`] ACL.
///
/// # Errors
/// Returns [`io::ErrorKind::AlreadyExists`] when anything already has that name, or another
/// local error.
pub(super) fn create_protected_dir(path: &Path) -> io::Result<()> {
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
    use super::create_protected_dir;

    #[test]
    fn a_staging_directory_is_protected_from_the_moment_it_exists()
    -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("ma2a-a2a-acl-{}", std::process::id()));
        let _stale = std::fs::remove_dir(&path);
        create_protected_dir(&path)?;
        assert!(
            create_protected_dir(&path)
                .is_err_and(|error| { error.kind() == std::io::ErrorKind::AlreadyExists })
        );
        // Before any other step: protected, and only the owner and SYSTEM.
        let status = std::process::Command::new("powershell.exe")
            .env_remove("PSModulePath")
            .env("MA2A_ACL_PATH", &path)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$a=Get-Acl -LiteralPath $env:MA2A_ACL_PATH;if(!$a.AreAccessRulesProtected){exit 2};$r=@($a.Access);if($r.Count-ne 2){exit 3};foreach($x in $r){$s=$x.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value;if(($s-ne 'S-1-3-4')-and($s-ne 'S-1-5-18')){exit 4}}",
            ])
            .status()?;
        std::fs::remove_dir(&path)?;
        assert!(status.success(), "{status}");
        Ok(())
    }
}
