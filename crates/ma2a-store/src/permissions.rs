use std::{fs, path::Path};

use crate::StoreError;

pub(crate) fn ensure_private_dir(path: &Path, target: &'static str) -> Result<(), StoreError> {
    validate_local_absolute(path)?;
    if !path.exists() {
        create_private_dir(path)?;
    }
    validate_private_path(path, target, PathKind::Directory)
}

/// Creates `path`, which must not exist yet, as a directory only this account may enter.
pub(crate) fn create_new_private_dir(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt as _;
        fs::DirBuilder::new().mode(0o700).create(path)?;
        Ok(())
    }
    #[cfg(windows)]
    {
        fs::create_dir(path)?;
        apply_windows_acl(path, PathKind::Directory)
    }
}

/// Restricts `path`, a regular file the caller just created, to the current account (and
/// SYSTEM on Windows, with no inherited entries), then confirms that it is.
///
/// # Errors
/// Returns [`StoreError`] when the permissions cannot be applied or confirmed.
pub fn protect_private_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    protect_new_file(path, target)
}

pub(crate) fn protect_new_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    protect_file(path)?;
    validate_private_path(path, target, PathKind::File)
}

pub(crate) fn validate_private_file(path: &Path, target: &'static str) -> Result<(), StoreError> {
    validate_private_path(path, target, PathKind::File)
}

fn validate_local_absolute(path: &Path) -> Result<(), StoreError> {
    if !path.is_absolute() || is_remote(path) {
        return Err(StoreError::InvalidStateDirectory);
    }
    Ok(())
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::DirBuilderExt as _;

    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)?;
    Ok(())
}

#[cfg(windows)]
fn create_private_dir(path: &Path) -> Result<(), StoreError> {
    fs::create_dir_all(path)?;
    apply_windows_acl(path, PathKind::Directory)
}

#[cfg(unix)]
fn protect_file(path: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt as _;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(windows)]
fn protect_file(path: &Path) -> Result<(), StoreError> {
    apply_windows_acl(path, PathKind::File)
}

#[cfg(unix)]
fn validate_private_path(
    path: &Path,
    target: &'static str,
    kind: PathKind,
) -> Result<(), StoreError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let metadata = fs::symlink_metadata(path)?;
    let expected_type = match kind {
        PathKind::Directory => metadata.is_dir(),
        PathKind::File => metadata.is_file(),
    };
    if metadata.file_type().is_symlink() || !expected_type {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "path is a symlink or has the wrong file type",
        });
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "path is not owned by the current user",
        });
    }
    let expected_mode = match kind {
        PathKind::Directory => 0o700,
        PathKind::File => 0o600,
    };
    if metadata.permissions().mode() & 0o777 != expected_mode {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "mode must be exactly owner-only",
        });
    }
    Ok(())
}

#[cfg(windows)]
fn validate_private_path(
    path: &Path,
    target: &'static str,
    kind: PathKind,
) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path)?;
    let expected_type = match kind {
        PathKind::Directory => metadata.is_dir(),
        PathKind::File => metadata.is_file(),
    };
    if metadata.file_type().is_symlink() || !expected_type {
        return Err(StoreError::InsecurePermissions {
            target,
            detail: "path is a symlink or has the wrong file type",
        });
    }
    validate_windows_acl(path)
}

#[cfg(windows)]
fn apply_windows_acl(path: &Path, kind: PathKind) -> Result<(), StoreError> {
    let (acl_type, inheritance) = match kind {
        PathKind::Directory => ("DirectorySecurity", "ContainerInherit,ObjectInherit"),
        PathKind::File => ("FileSecurity", "None"),
    };
    run_powershell(
        path,
        &format!(
            "$u=[Security.Principal.WindowsIdentity]::GetCurrent().User;$s=New-Object Security.Principal.SecurityIdentifier('S-1-5-18');$a=New-Object Security.AccessControl.{acl_type};$a.SetAccessRuleProtection($true,$false);$i=[Security.AccessControl.InheritanceFlags]'{inheritance}';$p=[Security.AccessControl.PropagationFlags]::None;$t=[Security.AccessControl.AccessControlType]::Allow;$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($u,'FullControl',$i,$p,$t)));$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($s,'FullControl',$i,$p,$t)));$a.SetOwner($u);Set-Acl -LiteralPath $env:MA2A_ACL_PATH -AclObject $a"
        ),
        "application",
    )
}

/// Accepts exactly two explicit Allow rules, one for the current user and one for SYSTEM,
/// each granting every FullControl right, under a DACL that inherits nothing, on an object
/// owned by the current user, SYSTEM, or the Administrators group. Any other owner could
/// rewrite the DACL; Administrators can take ownership anyway, and elevated sessions may make
/// it the default owner of new objects.
#[cfg(windows)]
fn validate_windows_acl(path: &Path) -> Result<(), StoreError> {
    run_powershell(
        path,
        "$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$s='S-1-5-18';$f=[Security.AccessControl.FileSystemRights]::FullControl;$a=Get-Acl -LiteralPath $env:MA2A_ACL_PATH;if(!$a.AreAccessRulesProtected){exit 2};$o=$a.GetOwner([Security.Principal.SecurityIdentifier]).Value;if(($o-ne $u)-and($o-ne $s)-and($o-ne 'S-1-5-32-544')){exit 7};$r=@($a.Access|Where-Object {!$_.IsInherited});if($r.Count-ne 2){exit 3};$seen=@{};foreach($x in $r){$sid=$x.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value;if(($sid-ne $u)-and($sid-ne $s)){exit 4};if($seen.ContainsKey($sid)){exit 6};$seen[$sid]=$true;if($x.AccessControlType-ne 'Allow' -or (($x.FileSystemRights-band $f)-ne $f)){exit 5}}",
        "validation",
    )
}

#[cfg(windows)]
fn run_powershell(path: &Path, script: &str, operation: &'static str) -> Result<(), StoreError> {
    // Windows PowerShell 5.1 must find its own modules: a PSModulePath inherited from a
    // PowerShell 7 parent points it at modules it cannot load, Set-Acl among them. The path
    // travels in the environment: `-Command` joins every later argument into the script
    // text, so `$args` would be empty, and no quoting is needed this way.
    let status = std::process::Command::new("powershell.exe")
        .env_remove("PSModulePath")
        .env("MA2A_ACL_PATH", path)
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .status()?;
    if !status.success() {
        return Err(StoreError::WindowsAcl { operation });
    }
    Ok(())
}

#[cfg(not(windows))]
const fn is_remote(_path: &Path) -> bool {
    false
}

#[cfg(windows)]
fn is_remote(path: &Path) -> bool {
    use std::path::{Component, Prefix};

    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) | Prefix::DeviceNS(_)
            )
    )
}

#[derive(Clone, Copy)]
enum PathKind {
    Directory,
    File,
}

#[cfg(all(test, windows))]
mod windows_acl_tests {
    use std::path::Path;

    use super::{PathKind, apply_windows_acl, run_powershell, validate_windows_acl};
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
}
