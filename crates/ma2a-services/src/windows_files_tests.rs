//! Tests for the Windows file-handle helpers.

use std::os::windows::fs::OpenOptionsExt as _;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES, READ_CONTROL,
};

use super::{create_protected_dir, file_id, final_path, pin_directory, pin_private_directory};
use crate::windows_acl::{current_user_matcher, owner_and_dacl};

fn open_directory(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | READ_CONTROL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

#[test]
fn a_protected_directory_is_private_from_the_moment_it_exists()
-> Result<(), Box<dyn std::error::Error>> {
    let user = current_user_matcher()?;
    let base = std::env::temp_dir().join(format!("ma2a-acl-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base)?;
    let path = base.join("private");
    create_protected_dir(&path)?;
    assert!(
        create_protected_dir(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    let (private, inherited) = (open_directory(&path)?, open_directory(&base)?);
    let sddl = owner_and_dacl(&private)?;
    assert!(crate::sddl::is_private(&sddl, &user), "{sddl}");
    // The user's own SID text and its system alias, where it has one, both resolve to it.
    let full = crate::windows_security::current_process_user_sid()?.to_sddl()?;
    assert!(user(&full));
    assert!(!user("SY") || full == "S-1-5-18");
    assert!(!user("S-1-5-21-1-2-3-500"));
    // A directory made the ordinary way inherits from the temporary directory.
    let sddl = owner_and_dacl(&inherited)?;
    assert!(!crate::sddl::is_private(&sddl, &user), "{sddl}");
    // Identity and location come from the handle.
    assert_eq!(file_id(&private)?, file_id(&open_directory(&path)?)?);
    assert_ne!(file_id(&private)?, file_id(&inherited)?);
    assert_eq!(final_path(&private)?, std::fs::canonicalize(&path)?);
    drop((private, inherited));
    std::fs::remove_dir_all(&base)?;
    Ok(())
}

#[test]
fn only_a_private_directory_itself_is_pinned() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-pin-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base)?;
    let (private, junction) = (base.join("private"), base.join("junction"));
    create_protected_dir(&private)?;
    let linked = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .args([&junction, &private])
        .stdout(std::process::Stdio::null())
        .status()?;

    let pinned = pin_private_directory(&private);
    let moved = std::fs::rename(&private, base.join("moved"));
    let (through_link, shared) = (
        pin_private_directory(&junction),
        pin_private_directory(&base),
    );

    let was_pinned = pinned.map(drop);
    std::fs::remove_dir(&junction)?;
    std::fs::remove_dir_all(&base)?;
    assert!(linked.success());
    assert!(was_pinned.is_ok(), "{was_pinned:?}");
    assert!(moved.is_err(), "{moved:?}");
    assert!(through_link.is_err() && shared.is_err());
    Ok(())
}

#[test]
fn a_pinned_directory_and_its_parent_cannot_be_renamed() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-pin-rename-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    let parent = base.join("parent");
    let pinned = parent.join("pinned");
    std::fs::create_dir_all(&pinned)?;

    let pin = pin_directory(&pinned);
    let moved = std::fs::rename(&pinned, parent.join("moved"));
    let parent_moved = std::fs::rename(&parent, base.join("moved"));

    let was_pinned = pin.map(drop);
    std::fs::remove_dir_all(&base)?;
    assert!(was_pinned.is_ok(), "{was_pinned:?}");
    assert!(moved.is_err(), "{moved:?}");
    assert!(parent_moved.is_err(), "{parent_moved:?}");
    Ok(())
}
