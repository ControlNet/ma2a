use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
};

use super::{
    PathKind, apply_windows_acl, validate_local_absolute, validate_private_file, windows_acl,
    windows_sid,
};
use crate::StoreError;

/// Replaces the file's DACL with protected rules: the current user with `rights`, and
/// SYSTEM with full control when `system`.
fn set_rules(path: &Path, rights: u32, system: bool) -> std::io::Result<()> {
    let (user, local_system) = (windows_sid::current_user()?, windows_sid::local_system()?);
    let mut rules = vec![(rights, &user)];
    if system {
        rules.push((FILE_ALL_ACCESS, &local_system));
    }
    windows_acl::set_owner_and_dacl(&windows_acl::open(path, true)?, (&user, 0), &rules)
}

#[test]
fn only_full_control_for_the_user_and_system_counts_as_private()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!("ma2a-acl-{}", std::process::id()));
    std::fs::write(&path, b"")?;
    // A file made the ordinary way inherits from the temporary directory.
    assert!(validate_private_file(&path, "test").is_err());
    apply_windows_acl(&path, PathKind::File)?;
    assert!(validate_private_file(&path, "test").is_ok());
    // Partial rights for the user, and a missing SYSTEM rule, are refused.
    set_rules(&path, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE, true)?;
    assert!(validate_private_file(&path, "test").is_err());
    set_rules(&path, FILE_ALL_ACCESS, false)?;
    assert!(validate_private_file(&path, "test").is_err());
    set_rules(&path, FILE_ALL_ACCESS, true)?;
    assert!(validate_private_file(&path, "test").is_ok());
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
