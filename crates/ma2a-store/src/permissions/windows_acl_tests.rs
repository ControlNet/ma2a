use std::path::Path;

use windows_sys::Win32::{
    Security::{CONTAINER_INHERIT_ACE, INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE},
    Storage::FileSystem::{FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ},
};

use super::{
    PathKind, apply_windows_acl, ensure_private_dir, validate_local_absolute,
    validate_private_file, validate_private_path, windows_acl, windows_sid,
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
    apply_windows_acl(&path, "test", PathKind::File)?;
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
fn a_drive_letter_mapped_to_a_network_share_is_refused_before_anything_is_created()
-> Result<(), Box<dyn std::error::Error>> {
    let subst = |args: &[&str]| std::process::Command::new("subst").args(args).status();
    // A free drive letter, mapped to the administrative share of this machine's C: drive.
    let drive = (b'G'..=b'Y')
        .rev()
        .map(|letter| format!("{}:", char::from(letter)))
        .find(|drive| !Path::new(&format!(r"{drive}\")).exists())
        .ok_or("no free drive letter")?;
    assert!(subst(&[&drive, r"\\localhost\C$"])?.success());
    let name = format!("ma2a-acl-mapped-{}", std::process::id());
    let result = ensure_private_dir(Path::new(&format!(r"{drive}\{name}\state")), "test");
    let unmapped = subst(&[&drive, "/D"])?;
    let created = Path::new(r"C:\").join(&name);
    let was_created = created.exists();
    let _cleanup = std::fs::remove_dir_all(&created);
    assert!(unmapped.success());
    assert!(
        matches!(result, Err(StoreError::InvalidStateDirectory)),
        "{result:?}"
    );
    assert!(!was_created);
    Ok(())
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

#[test]
fn a_junction_is_refused_as_a_link_and_never_protected() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-acl-junction-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    let (target, link) = (base.join("target"), base.join("link"));
    std::fs::create_dir_all(&target)?;
    apply_windows_acl(&target, "test", PathKind::Directory)?;
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .args([&link, &target])
        .stdout(std::process::Stdio::null())
        .status()?;
    assert!(status.success(), "mklink /J failed: {status}");

    let validated = ensure_private_dir(&link, "test");
    assert!(
        matches!(validated, Err(StoreError::InsecurePermissions { .. })),
        "{validated:?}"
    );
    let applied = apply_windows_acl(&link, "test", PathKind::Directory);
    assert!(
        matches!(applied, Err(StoreError::InsecurePermissions { .. })),
        "{applied:?}"
    );
    std::fs::remove_dir(&link)?;
    std::fs::remove_dir_all(&base)?;
    Ok(())
}
