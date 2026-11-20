//! Where a download may keep its helper files: never in a directory another account can enter,
//! never under its own destination's name, and never shared by destinations a case-sensitive
//! file system keeps apart.

#[path = "support/host.rs"]
mod support;

use std::io::ErrorKind;

use ma2a_core::service::ContentIdentity;
use ma2a_services::client::resume::{PRIVATE_DIRECTORY, Partial, partial_path};
use support::{TempRoot, TestResult};
use tokio::io::AsyncWriteExt as _;

#[tokio::test]
async fn an_existing_directory_others_can_enter_is_not_used() -> TestResult {
    // Given: the operator's own directory with the private directory's name, open to others:
    // on Windows a directory made plainly inherits its parent's entries.
    let local = TempRoot::new("download-open-directory")?;
    let directory = local.path().join(PRIVATE_DIRECTORY);
    std::fs::create_dir(&directory)?;
    #[cfg(unix)]
    std::fs::set_permissions(
        &directory,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )?;

    // When
    let opened = Partial::open(&local.path().join("report"), "peer docs:/file", false).await;

    // Then: the download is refused, and the directory stays in place, as it was.
    assert_eq!(
        opened.err().map(|error| error.kind()),
        Some(ErrorKind::AlreadyExists)
    );
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&directory)?.permissions())
            & 0o777,
        0o755
    );
    assert_eq!(std::fs::read_dir(&directory)?.count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_destination_named_like_the_private_directory_is_refused_up_front() -> TestResult {
    // Given: an empty directory.
    let local = TempRoot::new("download-reserved-name")?;
    for name in [PRIVATE_DIRECTORY, ".MA2A-Downloads"] {
        let destination = local.path().join(name);

        // When
        let opened = Partial::open(&destination, "peer docs:/file", false).await;

        // Then: refused before anything is created.
        assert_eq!(
            opened.err().map(|error| error.kind()),
            Some(ErrorKind::InvalidInput)
        );
        assert_eq!(std::fs::read_dir(local.path())?.count(), 0, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn destinations_differing_only_in_case_keep_their_own_progress() -> TestResult {
    // Given: a download to `report` interrupted after a prefix.
    let local = TempRoot::new("download-case")?;
    let (lower, upper) = (local.path().join("report"), local.path().join("REPORT"));
    assert_ne!(partial_path(&lower)?, partial_path(&upper)?);
    let mut first = Partial::open(&lower, "peer docs:/report", true).await?;
    first.begin(ContentIdentity::new(100, 1)).await?;
    first.sink().0.write_all(b"prefix").await?;
    first.sink().0.flush().await?;
    drop(first);

    // When: a fresh download to `REPORT` runs to completion meanwhile.
    let mut other = Partial::open(&upper, "peer docs:/other", false).await?;
    other.sink().0.write_all(b"other").await?;
    other.finish(true).await?;

    // Then: `report` still resumes from its own prefix.
    let resumed = Partial::open(&lower, "peer docs:/report", true).await?;
    assert_eq!(resumed.offset(), 6);
    resumed.discard().await?;
    Ok(())
}

/// Whether the file system holding `directory` names one file by two spellings that differ
/// only in ASCII case.
fn case_insensitive(directory: &std::path::Path) -> std::io::Result<bool> {
    std::fs::write(directory.join("case-probe"), b"")?;
    let insensitive = directory.join("CASE-PROBE").exists();
    std::fs::remove_file(directory.join("case-probe"))?;
    Ok(insensitive)
}

#[tokio::test]
async fn one_destination_has_one_writer_by_any_spelling_the_volume_accepts() -> TestResult {
    // Given: a download to `report` in progress.
    let local = TempRoot::new("download-case-lease")?;
    let held = Partial::open(&local.path().join("report"), "peer docs:/report", false).await?;

    // When: another spelling starts.
    let other = Partial::open(&local.path().join("Report"), "peer docs:/report", false).await;

    // Then: where the volume names one file by both, it waits for the first; where they are
    // two files, it is a download of its own.
    if case_insensitive(local.path())? {
        assert_eq!(
            other.err().map(|error| error.kind()),
            Some(ErrorKind::WouldBlock)
        );
    } else {
        other?.discard().await?;
    }
    held.discard().await?;
    Ok(())
}

#[tokio::test]
async fn a_private_directory_an_operator_made_is_kept() -> TestResult {
    // Given: the operator's own private directory, made before any download.
    let local = TempRoot::new("download-operator-directory")?;
    let directory = local.path().join(PRIVATE_DIRECTORY);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut std::fs::DirBuilder::new(), 0o700)
        .create(&directory)?;
    #[cfg(windows)]
    ma2a_services::windows_files::create_protected_dir(&directory)?;

    // When: one download completes and another is abandoned.
    let mut done = Partial::open(&local.path().join("done"), "peer docs:/done", false).await?;
    done.sink().0.write_all(b"done").await?;
    done.finish(false).await?;
    let abandoned = Partial::open(&local.path().join("gone"), "peer docs:/gone", false).await?;
    abandoned.discard().await?;

    // Then: the directory stays, empty, while one a download made goes when it ends.
    assert_eq!(std::fs::read_dir(&directory)?.count(), 0);
    let other = TempRoot::new("download-own-directory")?;
    let mut own = Partial::open(&other.path().join("done"), "peer docs:/done", false).await?;
    own.sink().0.write_all(b"done").await?;
    own.finish(false).await?;
    assert!(!other.path().join(PRIVATE_DIRECTORY).exists());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_download_whose_directory_was_swapped_publishes_nothing_and_keeps_both() -> TestResult {
    // Given: two downloads to `report`, one in `a` and one in `b`, each holding a prefix.
    let local = TempRoot::new("download-rotation")?;
    let (a, b) = (local.path().join("a"), local.path().join("b"));
    std::fs::create_dir(&a)?;
    std::fs::create_dir(&b)?;
    let mut first = Partial::open(&a.join("report"), "peer docs:/first", false).await?;
    first.sink().0.write_all(b"first").await?;
    let mut second = Partial::open(&b.join("report"), "peer docs:/second", false).await?;
    second.begin(ContentIdentity::new(100, 1)).await?;
    second.sink().0.write_all(b"second").await?;
    second.sink().0.flush().await?;

    // When: `a` is moved away and `b` takes its place, then the first download publishes.
    let moved = local.path().join("a2");
    std::fs::rename(&a, &moved)?;
    std::fs::rename(&b, &a)?;
    let published = first.finish(false).await;

    // Then: nothing is published anywhere, the first keeps its progress where its directory
    // went, and the second's files are exactly as they were.
    assert!(published.is_err());
    assert!(!a.join("report").exists());
    assert!(!moved.join("report").exists());
    assert_eq!(
        std::fs::read(partial_path(&moved.join("report"))?)?,
        b"first"
    );
    assert_eq!(std::fs::read(partial_path(&a.join("report"))?)?, b"second");
    drop(second);
    let resumed = Partial::open(&a.join("report"), "peer docs:/second", true).await?;
    assert_eq!(resumed.offset(), 6);
    resumed.discard().await?;
    Ok(())
}

#[cfg(windows)]
#[tokio::test]
async fn a_download_whose_junction_was_repointed_publishes_nothing_and_keeps_both() -> TestResult {
    // Given: a download to `report` through the junction `link` to `a`, and another to
    // `report` in `b`, each holding a prefix.
    let local = TempRoot::new("download-junction")?;
    let (a, b, link) = (
        local.path().join("a"),
        local.path().join("b"),
        local.path().join("link"),
    );
    std::fs::create_dir(&a)?;
    std::fs::create_dir(&b)?;
    junction(&link, &a)?;
    let mut first = Partial::open(&link.join("report"), "peer docs:/first", false).await?;
    first.sink().0.write_all(b"first").await?;
    let mut second = Partial::open(&b.join("report"), "peer docs:/second", false).await?;
    second.begin(ContentIdentity::new(100, 1)).await?;
    second.sink().0.write_all(b"second").await?;
    second.sink().0.flush().await?;

    // When: the junction is pointed at `b`, then the first download publishes.
    std::fs::remove_dir(&link)?;
    junction(&link, &b)?;
    let published = first.finish(false).await;

    // Then: nothing is published anywhere, and each keeps its own progress.
    assert!(published.is_err());
    assert!(!a.join("report").exists());
    assert!(!b.join("report").exists());
    assert_eq!(std::fs::read(partial_path(&a.join("report"))?)?, b"first");
    drop(second);
    assert_eq!(std::fs::read(partial_path(&b.join("report"))?)?, b"second");
    let resumed = Partial::open(&b.join("report"), "peer docs:/second", true).await?;
    assert_eq!(resumed.offset(), 6);
    resumed.discard().await?;
    Ok(())
}

/// Makes `link` a junction to `target`; unlike a symbolic link, it needs no privilege.
#[cfg(windows)]
fn junction(link: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cmd")
        .arg("/C")
        .arg("mklink")
        .arg("/J")
        .arg(link)
        .arg(target)
        .stdout(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("mklink /J failed: {status}")))
    }
}
