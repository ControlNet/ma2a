//! A download keeps its helper files in a private directory beside the destination, under
//! names derived from the destination, and still claims only the helper files it can show it
//! wrote: an unrelated file keeps its content, whether the download starts or not.

#[path = "support/host.rs"]
mod support;

use std::{io::ErrorKind, path::Path};

use ma2a_services::client::resume::{PRIVATE_DIRECTORY, Partial, partial_path};
use support::{TempRoot, TestResult};
use tokio::io::AsyncWriteExt as _;

/// Starts a fresh download to `destination` and returns how it was refused, if it was.
async fn refusal(destination: &Path) -> Option<ErrorKind> {
    match Partial::open(destination, "peer docs:/file", false).await {
        Ok(partial) => {
            let _discarded = partial.discard().await;
            None
        }
        Err(error) => Some(error.kind()),
    }
}

/// Creates the private directory beside `local` as an operator might: owner-only, but without
/// the marker of one a download made, so downloads keep it.
fn private_directory(local: &Path) -> std::io::Result<()> {
    // On Windows a directory made plainly inherits its parent's ACL, which a download refuses.
    #[cfg(windows)]
    return match ma2a_services::windows_files::create_protected_dir(&local.join(PRIVATE_DIRECTORY))
    {
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        created => created,
    };
    #[cfg(not(windows))]
    {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        builder.create(local.join(PRIVATE_DIRECTORY))
    }
}

/// Downloads `content` to `destination` and publishes it.
async fn download(destination: &Path, content: &[u8]) -> TestResult {
    let mut partial = Partial::open(destination, "peer docs:/file", false).await?;
    partial.sink().0.write_all(content).await?;
    partial.finish(false).await?;
    Ok(())
}

#[tokio::test]
async fn files_named_like_a_destination_plus_a_helper_suffix_are_never_touched() -> TestResult {
    // Given: the operator's files, empty or not, named like the destination plus a suffix.
    let local = TempRoot::new("sidecar-neighbours")?;
    let neighbours = [
        ("report.ma2a-part", &b""[..]),
        ("report.ma2a-part.meta", b"ma2a-download v1\nnotes"),
        ("report.ma2a-part.meta.tmp", b"draft"),
        ("report.ma2a-part.lock", b""),
    ];
    for (name, content) in neighbours {
        std::fs::write(local.path().join(name), content)?;
    }

    // When
    download(&local.path().join("report"), b"downloaded").await?;

    // Then: the download completed beside them and every one of them is as it was.
    assert_eq!(std::fs::read(local.path().join("report"))?, b"downloaded");
    for (name, content) in neighbours {
        assert_eq!(std::fs::read(local.path().join(name))?, content, "{name}");
    }
    assert!(!local.path().join(PRIVATE_DIRECTORY).exists());
    Ok(())
}

#[tokio::test]
async fn a_destination_named_like_another_downloads_helper_has_its_own_helpers() -> TestResult {
    // Given: two downloads at once, one to a name the other once used for its partial file.
    let local = TempRoot::new("sidecar-overlap")?;
    let (report, helper_named) = (
        local.path().join("report"),
        local.path().join("report.ma2a-part"),
    );
    assert_ne!(partial_path(&report)?, partial_path(&helper_named)?);
    let mut first = Partial::open(&report, "peer docs:/report", false).await?;
    let mut second = Partial::open(&helper_named, "peer docs:/other", false).await?;

    // When
    first.sink().0.write_all(b"first").await?;
    second.sink().0.write_all(b"second").await?;
    first.finish(false).await?;
    second.finish(false).await?;

    // Then: each destination holds its own download.
    assert_eq!(std::fs::read(&report)?, b"first");
    assert_eq!(std::fs::read(&helper_named)?, b"second");
    Ok(())
}

#[tokio::test]
async fn an_unrelated_file_inside_the_private_directory_is_still_kept() -> TestResult {
    // Given: something without the MA2A header where this download's sidecar would be.
    let local = TempRoot::new("sidecar-meta")?;
    let destination = local.path().join("report");
    let sidecar = partial_path(&destination)?.with_extension("ma2a-part.meta");
    private_directory(local.path())?;
    std::fs::write(&sidecar, b"notes")?;

    // When
    let refused = refusal(&destination).await;

    // Then
    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&sidecar)?, b"notes");
    Ok(())
}

#[tokio::test]
async fn a_file_named_like_the_private_directory_is_kept() -> TestResult {
    let local = TempRoot::new("sidecar-directory")?;
    let unrelated = local.path().join(PRIVATE_DIRECTORY);
    std::fs::write(&unrelated, b"keep me")?;

    let refused = refusal(&local.path().join("report")).await;

    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&unrelated)?, b"keep me");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_where_the_partial_file_would_be_is_never_followed() -> TestResult {
    let local = TempRoot::new("sidecar-link")?;
    let destination = local.path().join("report");
    let target = local.path().join("elsewhere");
    std::fs::write(&target, b"")?;
    private_directory(local.path())?;
    std::os::unix::fs::symlink(&target, partial_path(&destination)?)?;

    let refused = refusal(&destination).await;

    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn the_private_directory_is_readable_only_by_this_user() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let local = TempRoot::new("sidecar-mode")?;
    let partial = Partial::open(&local.path().join("report"), "peer docs:/file", false).await?;

    let mode = std::fs::metadata(local.path().join(PRIVATE_DIRECTORY))?
        .permissions()
        .mode();

    partial.discard().await?;
    assert_eq!(mode & 0o777, 0o700);
    Ok(())
}

#[tokio::test]
async fn helper_files_this_download_left_behind_are_still_reclaimed() -> TestResult {
    // Given: an interrupted download left its sidecar, a partial file, and its lock file.
    let local = TempRoot::new("sidecar-own")?;
    let destination = local.path().join("report");
    let mut first = Partial::open(&destination, "peer docs:/file", false).await?;
    first
        .begin(ma2a_core::service::ContentIdentity::new(100, 1))
        .await?;
    first.sink().0.write_all(b"prefix").await?;
    drop(first);

    // When / Then: a fresh download starts over from them, and tidies up when it ends.
    let fresh = Partial::open(&destination, "peer docs:/file", false).await?;
    assert_eq!(fresh.offset(), 0);
    fresh.discard().await?;
    assert!(!local.path().join(PRIVATE_DIRECTORY).exists());
    Ok(())
}

#[tokio::test]
async fn a_staged_sidecar_cut_short_by_an_interruption_is_reclaimed() -> TestResult {
    // Given: a download stopped while writing its staged sidecar, at two different points.
    let local = TempRoot::new("sidecar-staged-cut")?;
    for (name, written) in [("empty", &b""[..]), ("prefix", &b"ma2a-dow"[..])] {
        private_directory(local.path())?;
        let destination = local.path().join(name);
        let staged = partial_path(&destination)?.with_extension("ma2a-part.meta.tmp");
        std::fs::write(staged, written)?;

        // When / Then: the next download starts instead of refusing its own file, and takes
        // the staged file with it when it ends, leaving the directory it did not make empty.
        assert_eq!(refusal(&destination).await, None, "{name}");
        let left = std::fs::read_dir(local.path().join(PRIVATE_DIRECTORY))?.count();
        assert_eq!(left, 0, "{name}");
    }
    Ok(())
}

#[tokio::test]
async fn progress_still_buffered_counts_as_bytes_held() -> TestResult {
    let local = TempRoot::new("sidecar-progress")?;
    let destination = local.path().join("file.bin");
    let mut partial = Partial::open(&destination, "peer docs:/file", false).await?;
    assert!(!partial.has_bytes().await?);
    // Written but not yet flushed: the check writes it out before reading the length.
    partial.sink().0.write_all(b"prefix").await?;
    assert!(partial.has_bytes().await?);
    assert_eq!(std::fs::read(partial_path(&destination)?)?, b"prefix");
    partial.discard().await?;
    Ok(())
}

#[tokio::test]
async fn kept_content_is_never_continued_with_content_of_another_identity() -> TestResult {
    use ma2a_core::service::ContentIdentity;

    // Given: a download that kept a prefix of one identity.
    let local = TempRoot::new("sidecar-identity")?;
    let destination = local.path().join("report");
    let mut first = Partial::open(&destination, "peer docs:/file", false).await?;
    first.begin(ContentIdentity::new(100, 1)).await?;
    first.sink().0.write_all(b"prefix").await?;
    first.sink().0.flush().await?;
    drop(first);

    // When: its resumption is answered with another identity.
    let mut resumed = Partial::open(&destination, "peer docs:/file", true).await?;
    assert_eq!(resumed.offset(), 6);
    let other = resumed.begin(ContentIdentity::new(200, 2)).await;

    // Then: it is refused, and the kept prefix still belongs to the first identity.
    assert_eq!(
        other.map_err(|error| error.kind()),
        Err(ErrorKind::InvalidData)
    );
    drop(resumed);
    let again = Partial::open(&destination, "peer docs:/file", true).await?;
    assert_eq!(again.identity(), Some(ContentIdentity::new(100, 1)));
    again.discard().await?;
    Ok(())
}
