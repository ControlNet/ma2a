use std::path::Path;

use windows_sys::Win32::{
    Security::{CONTAINER_INHERIT_ACE, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE},
    Storage::FileSystem::{FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ},
};

use super::{
    PathKind, apply_windows_acl, validate_local_absolute, validate_private_file,
    validate_private_path, windows_acl, windows_sid,
};
use crate::StoreError;

/// Replaces the file's DACL with protected rules: the current user with `rights`, and
/// SYSTEM with full control when `system`.
fn set_rules(path: &Path, rights: u32, system: bool) -> std::io::Result<()> {
    let (user, local_system) = (windows_sid::current_user()?, windows_sid::local_system()?);
    let mut rules = vec![(rights, 0, &user)];
    if system {
        rules.push((FILE_ALL_ACCESS, 0, &local_system));
    }
    windows_acl::set_owner_and_dacl(&windows_acl::open(path, true)?, &user, &rules)
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
    // A missing SYSTEM rule, and partial rights for the user, are refused. Partial rights come
    // last: they take away the right to change the owner again, and without the backup and
    // restore privileges nothing can rewrite the rules after them. The temporary directory
    // still lets the user delete the file.
    set_rules(&path, FILE_ALL_ACCESS, false)?;
    assert!(validate_private_file(&path, "test").is_err());
    set_rules(&path, FILE_ALL_ACCESS, true)?;
    assert!(validate_private_file(&path, "test").is_ok());
    set_rules(&path, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE, true)?;
    assert!(validate_private_file(&path, "test").is_err());
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

#[test]
fn a_rule_for_children_alone_is_not_full_control_over_the_directory()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!("ma2a-acl-children-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&path);
    std::fs::create_dir(&path)?;
    let (user, system) = (windows_sid::current_user()?, windows_sid::local_system()?);
    let inherit = CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE;
    let set_system_flags = |flags| {
        let rules = [
            (FILE_ALL_ACCESS, inherit, &user),
            (FILE_ALL_ACCESS, flags, &system),
        ];
        windows_acl::set_owner_and_dacl(&windows_acl::open(&path, true)?, &user, &rules)
    };
    // SYSTEM's full control applies to the directory's children only.
    set_system_flags(inherit | INHERIT_ONLY_ACE)?;
    assert!(validate_private_path(&path, "test", PathKind::Directory).is_err());
    // The same rule, applying to the directory too, is the private ACL the Store sets.
    set_system_flags(inherit)?;
    validate_private_path(&path, "test", PathKind::Directory)?;
    std::fs::remove_dir(&path)?;
    Ok(())
}
