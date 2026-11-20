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

#[cfg(unix)]
#[tokio::test]
async fn an_existing_directory_others_can_enter_is_not_used() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    // Given: the operator's own directory with the private directory's name, open to others.
    let local = TempRoot::new("download-open-directory")?;
    let directory = local.path().join(PRIVATE_DIRECTORY);
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))?;

    // When
    let opened = Partial::open(&local.path().join("report"), "peer docs:/file", false).await;

    // Then: the download is refused, and the directory keeps its mode and stays in place.
    assert_eq!(
        opened.err().map(|error| error.kind()),
        Some(ErrorKind::AlreadyExists)
    );
    let mode = std::fs::metadata(&directory)?.permissions().mode();
    assert_eq!(mode & 0o777, 0o755);
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

#[tokio::test]
async fn destinations_differing_only_in_case_never_have_two_writers() -> TestResult {
    // Given: a download to `report` in progress.
    let local = TempRoot::new("download-case-lease")?;
    let held = Partial::open(&local.path().join("report"), "peer docs:/report", false).await?;

    // When: another spelling, which a case-insensitive volume resolves to the same file, starts.
    let other = Partial::open(&local.path().join("Report"), "peer docs:/report", false).await;

    // Then: it waits for the first rather than writing alongside it.
    assert_eq!(
        other.err().map(|error| error.kind()),
        Some(ErrorKind::WouldBlock)
    );
    held.discard().await?;
    Ok(())
}
