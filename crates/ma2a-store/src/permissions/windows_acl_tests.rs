use std::path::Path;

use super::{
    PathKind, apply_windows_acl, run_powershell, validate_local_absolute, validate_windows_acl,
};
use crate::StoreError;

/// Replaces the file's DACL with protected rules: the current user with `rights`, and
/// SYSTEM with full control when `system`.
fn set_rules(path: &Path, rights: &str, system: bool) -> Result<(), StoreError> {
    let system = if system {
        "$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($s,'FullControl','Allow')));"
    } else {
        ""
    };
    run_powershell(
        path,
        &format!(
            "$u=[Security.Principal.WindowsIdentity]::GetCurrent().User;$s=New-Object Security.Principal.SecurityIdentifier('S-1-5-18');$a=New-Object Security.AccessControl.FileSecurity;$a.SetAccessRuleProtection($true,$false);$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($u,'{rights}','Allow')));{system}Set-Acl -LiteralPath $env:MA2A_ACL_PATH -AclObject $a"
        ),
        "test fixture",
    )
}

#[test]
fn only_full_control_for_the_user_and_system_counts_as_private()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!("ma2a-acl-{}", std::process::id()));
    std::fs::write(&path, b"")?;
    apply_windows_acl(&path, PathKind::File)?;
    assert!(validate_windows_acl(&path).is_ok());
    // Partial rights for the user, and a missing SYSTEM rule, are refused.
    set_rules(&path, "ReadAndExecute", true)?;
    assert!(validate_windows_acl(&path).is_err());
    set_rules(&path, "FullControl", false)?;
    assert!(validate_windows_acl(&path).is_err());
    set_rules(&path, "FullControl", true)?;
    assert!(validate_windows_acl(&path).is_ok());
    std::fs::remove_file(&path)?;
    Ok(())
}

#[test]
fn only_a_path_on_a_drive_letter_counts_as_local() {
    assert!(validate_local_absolute(Path::new(r"C:\state")).is_ok());
    assert!(validate_local_absolute(Path::new(r"\\?\C:\state")).is_ok());
    for remote in [
        r"\\server\share\state",
        r"\\?\UNC\server\share\state",
        r"\\?\GLOBALROOT\Device\Mup\server\share\state",
        r"\\.\pipe\state",
    ] {
        assert!(
            matches!(
                validate_local_absolute(Path::new(remote)),
                Err(StoreError::InvalidStateDirectory)
            ),
            "{remote}"
        );
    }
}
