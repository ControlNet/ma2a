//! Tests for writing invitation ticket files.

use super::check_destination;

#[test]
fn only_a_new_file_in_an_existing_directory_is_accepted() -> Result<(), std::io::Error> {
    let directory = std::env::temp_dir().join(format!("ma2a-invite-check-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("taken"), b"")?;
    let path = |name: &str| directory.join(name).to_string_lossy().into_owned();

    assert!(check_destination(&path("ticket")).is_ok());
    assert!(check_destination(&path("taken")).is_err());
    assert!(check_destination(&path("missing/ticket")).is_err());
    assert!(check_destination(&path("taken/ticket")).is_err());
    assert!(check_destination("").is_err());
    assert!(check_destination(&path("..")).is_err());
    assert!(check_destination(&path("ti\0cket")).is_err());
    assert!(check_destination(&format!("{}/", path("ticket"))).is_err());
    assert!(check_destination(&format!("{}/.", path("ticket"))).is_err());
    assert!(check_destination(&path(&"t".repeat(300))).is_err());
    std::fs::remove_dir_all(&directory)
}

#[test]
fn a_ticket_is_published_once_and_no_staging_is_left() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-invite-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base)?;
    let output = base.join("ticket");
    let path = output.to_str().ok_or("path")?;
    super::Destination::open(path)?.write(b"secret")?;
    assert_eq!(std::fs::read(&output)?, b"secret\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&output)?.permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // An existing file is never replaced, and no staging entry is left either way.
    assert!(super::Destination::open(path).is_err());
    assert_eq!(std::fs::read(&output)?, b"secret\n");
    let names = std::fs::read_dir(&base)?
        .map(|entry| Ok(entry?.file_name()))
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(names, ["ticket"]);
    std::fs::remove_dir_all(&base)?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_ticket_stays_in_its_directory_when_the_directory_is_renamed()
-> Result<(), Box<dyn std::error::Error>> {
    // Given: the ticket is staged in its output directory.
    let base = std::env::temp_dir().join(format!("ma2a-invite-moved-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    let (output, retired) = (base.join("output"), base.join("retired"));
    std::fs::create_dir_all(&output)?;
    let path = output.join("ticket");
    let path = path.to_str().ok_or("path")?;
    let (directory, name) = super::destination_directory(path)?;
    let staged = super::fill(&directory, super::create_staging(&directory)?, b"secret")?;

    // When: the directory is rotated away and an empty one takes its name before publication.
    std::fs::rename(&output, &retired)?;
    std::fs::create_dir(&output)?;
    let published = super::publish_staged(&directory, &staged, name);

    // Then: the ticket is published in the directory it was staged in, with no staging left.
    let names = |directory: &std::path::Path| {
        std::fs::read_dir(directory)?
            .map(|entry| Ok(entry?.file_name()))
            .collect::<std::io::Result<Vec<_>>>()
    };
    let (moved, replacement) = (names(&retired)?, names(&output)?);
    let ticket = std::fs::read(retired.join("ticket"));
    std::fs::remove_dir_all(&base)?;
    assert!(published.is_ok(), "{published:?}");
    assert_eq!(moved, ["ticket"]);
    assert!(replacement.is_empty(), "{replacement:?}");
    assert_eq!(ticket?, b"secret\n");
    Ok(())
}

/// An owner may create a named file in a directory it can write and search but not list.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn a_ticket_is_published_into_a_write_and_search_only_directory()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    let base = std::env::temp_dir().join(format!("ma2a-invite-wx-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base)?;
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o300))?;
    let output = base.join("ticket");
    let path = output.to_str().ok_or("path")?;

    let written = super::Destination::open(path).and_then(|ticket| ticket.write(b"secret"));

    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
    let ticket = std::fs::read(&output);
    std::fs::remove_dir_all(&base)?;
    assert!(written.is_ok(), "{written:?}");
    assert_eq!(ticket?, b"secret\n");
    Ok(())
}

#[cfg(unix)]
#[test]
fn publishing_claims_a_free_name_and_never_replaces_a_file()
-> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::temp_dir().join(format!("ma2a-invite-publish-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base)?;
    let destination = base.join("ticket");
    let path = destination.to_str().ok_or("path")?;
    let (directory, name) = super::destination_directory(path)?;
    let staged = super::fill(&directory, super::create_staging(&directory)?, b"secret")?;
    super::publish_staged(&directory, &staged, name)?;
    assert_eq!(std::fs::read(&destination)?, b"secret\n");

    // A file someone else put at the name stays, whether linked or renamed over.
    std::fs::remove_file(&destination)?;
    std::fs::write(&destination, b"theirs")?;
    let staged = super::fill(&directory, super::create_staging(&directory)?, b"secret")?;
    assert!(super::rename_no_clobber(&directory, (&staged, name)).is_err());
    assert!(super::publish_staged(&directory, &staged, name).is_err());
    assert_eq!(std::fs::read(&destination)?, b"theirs");
    std::fs::remove_dir_all(&base)?;
    Ok(())
}

/// The staging file is made when the destination is opened, before the invitation is
/// committed, so a directory no file can be created in is refused then; one never written is
/// removed when the destination is dropped.
#[cfg(unix)]
#[test]
fn a_destination_stages_its_file_when_opened_and_removes_it_unless_written()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    let base = std::env::temp_dir().join(format!("ma2a-invite-open-{}", std::process::id()));
    let _stale = std::fs::remove_dir_all(&base);
    std::fs::create_dir(&base)?;
    let output = base.join("ticket");
    let path = output.to_str().ok_or("path")?;
    let opened = super::Destination::open(path)?;
    let staged = std::fs::read_dir(&base)?.count();
    drop(opened);
    let left = std::fs::read_dir(&base)?.count();
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o500))?;
    let unwritable = super::Destination::open(path);
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))?;
    std::fs::remove_dir_all(&base)?;
    assert_eq!((staged, left), (1, 0));
    assert!(unwritable.is_err());
    Ok(())
}
