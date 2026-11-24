//! Claiming and writing the adapter's token file.

use std::path::{Path, PathBuf};

use super::{Published, claim, remove_if_ours, write_private, write_staged};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A fresh directory for one test, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Result<Self, std::io::Error> {
        let path = std::env::temp_dir().join(format!("ma2a-a2a-{name}-{}", std::process::id()));
        let _stale = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

fn names(directory: &Path) -> Result<Vec<String>, std::io::Error> {
    let mut names = std::fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

#[test]
fn a_second_adapter_on_a_claimed_token_file_is_refused_until_the_first_stops() -> TestResult {
    let scratch = Scratch::new("claim")?;
    let path = scratch.path().join("a2a-token");

    let first = claim(&path).map_err(|error| error.to_string())?;
    let refused = claim(&path).err().map(|error| error.to_string());
    assert!(refused.is_some_and(|message| message.contains("--token-file")));
    assert!(claim(&scratch.path().join("other-token")).is_ok());
    // Another spelling of the same path is the same claim.
    assert!(claim(&scratch.path().join(".").join("a2a-token")).is_err());
    drop(first);
    assert!(claim(&path).is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn every_runtime_of_this_account_contends_for_one_token_file() -> TestResult {
    // Given: one adapter holding a token file, reached through two directory spellings, as two
    // Runtimes with different state directories would name it.
    let scratch = Scratch::new("shared")?;
    std::fs::create_dir(scratch.path().join("real"))?;
    std::os::unix::fs::symlink(scratch.path().join("real"), scratch.path().join("link"))?;
    let first = claim(&scratch.path().join("real").join("token")).map_err(|e| e.to_string())?;

    // Then: the second claim fails before anything is written.
    assert!(claim(&scratch.path().join("link").join("token")).is_err());
    drop(first);
    Ok(())
}

#[test]
fn names_the_adapter_keeps_for_itself_are_refused_as_token_files() -> TestResult {
    let scratch = Scratch::new("reserved")?;
    for name in [".ma2a-a2a-locks", ".MA2A-A2A-locks", ".ma2a-a2a-stage-00"] {
        assert!(claim(&scratch.path().join(name)).is_err(), "{name}");
    }
    let held = claim(&scratch.path().join("token")).map_err(|error| error.to_string())?;
    let inside = scratch.path().join(".ma2a-a2a-locks").join("other");
    assert!(claim(&inside).is_err());
    assert!(write_private(&inside, "x").is_err());
    drop(held);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_lock_directory_another_could_enter_is_refused_and_kept() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("open-locks")?;
    let locks = scratch.path().join(".ma2a-a2a-locks");
    std::fs::create_dir(&locks)?;
    std::fs::set_permissions(&locks, std::fs::Permissions::from_mode(0o755))?;

    assert!(claim(&scratch.path().join("token")).is_err());
    assert_eq!(
        std::fs::metadata(&locks)?.permissions().mode() & 0o777,
        0o755
    );
    Ok(())
}

#[test]
fn a_taken_staging_name_is_kept_and_the_next_one_used() -> TestResult {
    // Given: the first staging name is already somebody's file, and a file named like the
    // adapter's earlier per-process staging name sits beside the token file.
    let scratch = Scratch::new("staging")?;
    let taken = ".ma2a-a2a-stage-taken".to_owned();
    std::fs::write(scratch.path().join(&taken), "theirs")?;
    let earlier = format!("a2a-token.ma2a-partial-{}", std::process::id());
    std::fs::write(scratch.path().join(&earlier), "theirs too")?;
    let path = scratch.path().join("a2a-token");

    // When
    let candidates = [taken.clone(), ".ma2a-a2a-stage-free".to_owned()];
    write_staged(&path, "secret", candidates).map_err(|error| error.to_string())?;

    // Then: the token is published, both files are untouched, and nothing else is left.
    assert_eq!(std::fs::read_to_string(&path)?, "secret");
    assert_eq!(
        std::fs::read_to_string(scratch.path().join(&taken))?,
        "theirs"
    );
    assert_eq!(
        std::fs::read_to_string(scratch.path().join(&earlier))?,
        "theirs too"
    );
    assert_eq!(
        names(scratch.path())?,
        [taken, "a2a-token".to_owned(), earlier]
    );
    Ok(())
}

#[test]
fn no_free_staging_name_fails_and_removes_nothing() -> TestResult {
    let scratch = Scratch::new("no-staging")?;
    let taken = ".ma2a-a2a-stage-taken".to_owned();
    std::fs::write(scratch.path().join(&taken), "theirs")?;
    let path = scratch.path().join("a2a-token");

    assert!(write_staged(&path, "secret", [taken.clone()]).is_err());

    assert_eq!(names(scratch.path())?, [taken]);
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_published_token_is_owner_only() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;

    let scratch = Scratch::new("mode")?;
    let path = scratch.path().join("a2a-token");
    write_private(&path, "secret").map_err(|error| error.to_string())?;
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o600
    );
    Ok(())
}

#[test]
fn a_stopping_adapter_leaves_a_token_another_adapter_wrote_since() -> TestResult {
    let scratch = Scratch::new("token")?;
    let path = scratch.path().join("a2a-token");
    write_private(&path, "first").map_err(|error| error.to_string())?;
    write_private(&path, "second").map_err(|error| error.to_string())?;

    remove_if_ours(&path, "first");
    assert_eq!(std::fs::read_to_string(&path)?, "second");
    remove_if_ours(&path, "second");
    assert!(!path.exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_stopping_adapter_reads_no_link_or_fifo_in_place_of_its_token() -> TestResult {
    let scratch = Scratch::new("token-swapped")?;
    let path = scratch.path().join("a2a-token");
    let elsewhere = scratch.path().join("elsewhere");
    std::fs::write(&elsewhere, "ours")?;
    std::os::unix::fs::symlink(&elsewhere, &path)?;

    // A link to a file holding the token is not the token file.
    remove_if_ours(&path, "ours");
    assert!(std::fs::symlink_metadata(&path).is_ok());

    // Nor is a FIFO, and reading it would block the stop.
    std::fs::remove_file(&path)?;
    let made = std::process::Command::new("mkfifo").arg(&path).status()?;
    assert!(made.success());
    remove_if_ours(&path, "ours");
    assert!(std::fs::symlink_metadata(&path).is_ok());
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_claim_keeps_the_directory_its_path_resolved_to() -> TestResult {
    // Given: a token file claimed through a link to `a`.
    let scratch = Scratch::new("repointed")?;
    let (a, b, link) = (
        scratch.path().join("a"),
        scratch.path().join("b"),
        scratch.path().join("link"),
    );
    std::fs::create_dir(&a)?;
    std::fs::create_dir(&b)?;
    std::os::unix::fs::symlink(&a, &link)?;
    let held = claim(&link.join("token")).map_err(|error| error.to_string())?;

    // When: the link is pointed at `b`, then the token is written and the adapter stops.
    std::fs::remove_file(&link)?;
    std::os::unix::fs::symlink(&b, &link)?;
    write_private(held.path(), "secret").map_err(|error| error.to_string())?;

    // Then: the token is beside the lock it was claimed with, and `b` holds nothing.
    assert_eq!(std::fs::read(a.join("token"))?, b"secret");
    assert!(names(&b)?.is_empty());
    // A second adapter through the repointed link names another file and may claim it.
    assert!(claim(&link.join("token")).is_ok());
    drop(held);
    Ok(())
}

#[test]
fn a_published_token_is_removed_however_the_adapter_ends() -> TestResult {
    // Given: a published token file.
    let scratch = Scratch::new("published")?;
    let path = scratch.path().join("token");
    write_private(&path, "first").map_err(|error| error.to_string())?;

    // When / Then: dropping a guard for another token keeps it; its own guard removes it.
    drop(Published {
        path: &path,
        token: "other",
    });
    assert!(path.exists());
    drop(Published {
        path: &path,
        token: "first",
    });
    assert!(!path.exists());
    Ok(())
}
