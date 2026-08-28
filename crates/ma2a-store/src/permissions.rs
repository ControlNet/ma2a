use std::{fs, path::Path};

use crate::StoreError;

pub(crate) fn ensure_private_dir(path: &Path, target: &'static str) -> Result<(), StoreError> {
    validate_local_absolute(path)?;
    if !path.exists() {
        create_private_dir(path)?;
    }
    validate_private_path(path, target, PathKind::Directory)
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
            "$u=[Security.Principal.WindowsIdentity]::GetCurrent().User;$s=New-Object Security.Principal.SecurityIdentifier('S-1-5-18');$a=New-Object Security.AccessControl.{acl_type};$a.SetAccessRuleProtection($true,$false);$i=[Security.AccessControl.InheritanceFlags]'{inheritance}';$p=[Security.AccessControl.PropagationFlags]::None;$t=[Security.AccessControl.AccessControlType]::Allow;$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($u,'FullControl',$i,$p,$t)));$a.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($s,'FullControl',$i,$p,$t)));Set-Acl -LiteralPath $args[0] -AclObject $a"
        ),
        "application",
    )
}

#[cfg(windows)]
fn validate_windows_acl(path: &Path) -> Result<(), StoreError> {
    run_powershell(
        path,
        "$u=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value;$s='S-1-5-18';$a=Get-Acl -LiteralPath $args[0];if(!$a.AreAccessRulesProtected){exit 2};$r=@($a.Access|Where-Object {!$_.IsInherited});if($r.Count-ne 2){exit 3};foreach($x in $r){$sid=$x.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value;if(($sid-ne $u)-and($sid-ne $s)){exit 4};if($x.AccessControlType-ne 'Allow' -or (($x.FileSystemRights-band [Security.AccessControl.FileSystemRights]::FullControl)-eq 0)){exit 5}}",
        "validation",
    )
}

#[cfg(windows)]
fn run_powershell(path: &Path, script: &str, operation: &'static str) -> Result<(), StoreError> {
    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .arg(path)
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
