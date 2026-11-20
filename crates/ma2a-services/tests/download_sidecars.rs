//! A download claims only the helper files it can show it wrote: an unrelated file that
//! happens to carry a helper's name keeps its content, whether the download starts or not.

#[path = "support/host.rs"]
mod support;

use std::io::ErrorKind;

use ma2a_services::client::resume::Partial;
use support::{TempRoot, TestResult};

/// Starts a fresh download to `destination` and returns how it was refused, if it was.
async fn refusal(destination: &std::path::Path) -> Option<ErrorKind> {
    match Partial::open(destination, "peer docs:/file", false).await {
        Ok(partial) => {
            let _discarded = partial.discard().await;
            None
        }
        Err(error) => Some(error.kind()),
    }
}

#[tokio::test]
async fn an_unrelated_file_named_like_the_partial_file_is_kept() -> TestResult {
    // Given: a real file whose name is the partial-file name of another destination.
    let local = TempRoot::new("sidecar-part")?;
    let unrelated = local.path().join("report.ma2a-part");
    std::fs::write(&unrelated, b"someone's report")?;

    // When
    let refused = refusal(&local.path().join("report")).await;

    // Then
    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&unrelated)?, b"someone's report");
    assert!(!local.path().join("report.ma2a-part.lock").exists());
    Ok(())
}

#[tokio::test]
async fn an_unrelated_file_named_like_the_sidecar_is_kept() -> TestResult {
    let local = TempRoot::new("sidecar-meta")?;
    let unrelated = local.path().join("report.ma2a-part.meta");
    std::fs::write(&unrelated, b"notes")?;

    let refused = refusal(&local.path().join("report")).await;

    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&unrelated)?, b"notes");
    assert!(!local.path().join("report.ma2a-part.lock").exists());
    Ok(())
}

#[tokio::test]
async fn an_unrelated_file_named_like_the_staged_sidecar_is_kept() -> TestResult {
    let local = TempRoot::new("sidecar-staged")?;
    let unrelated = local.path().join("report.ma2a-part.meta.tmp");
    std::fs::write(&unrelated, b"draft")?;

    let refused = refusal(&local.path().join("report")).await;

    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&unrelated)?, b"draft");
    assert!(!local.path().join("report.ma2a-part.lock").exists());
    Ok(())
}

#[tokio::test]
async fn an_unrelated_file_named_like_the_lock_is_kept() -> TestResult {
    let local = TempRoot::new("sidecar-lock")?;
    let unrelated = local.path().join("report.ma2a-part.lock");
    std::fs::write(&unrelated, b"keep me")?;

    let refused = refusal(&local.path().join("report")).await;

    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
    assert_eq!(std::fs::read(&unrelated)?, b"keep me");
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_named_like_the_partial_file_is_never_followed() -> TestResult {
    let local = TempRoot::new("sidecar-link")?;
    let target = local.path().join("elsewhere");
    std::fs::write(&target, b"")?;
    std::os::unix::fs::symlink(&target, local.path().join("report.ma2a-part"))?;

    let refused = refusal(&local.path().join("report")).await;

    assert_eq!(refused, Some(ErrorKind::AlreadyExists));
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
    tokio::io::AsyncWriteExt::write_all(first.sink().0, b"prefix").await?;
    drop(first);

    // When / Then: a fresh download starts over from them.
    let fresh = Partial::open(&destination, "peer docs:/file", false).await?;
    assert_eq!(fresh.offset(), 0);
    fresh.discard().await?;
    assert!(!local.path().join("report.ma2a-part").exists());
    Ok(())
}

#[tokio::test]
async fn a_staged_sidecar_cut_short_by_an_interruption_is_reclaimed() -> TestResult {
    // Given: a download stopped while writing its staged sidecar, at two different points.
    let local = TempRoot::new("sidecar-staged-cut")?;
    for (name, written) in [("empty", &b""[..]), ("prefix", &b"ma2a-dow"[..])] {
        let destination = local.path().join(name);
        std::fs::write(
            local.path().join(format!("{name}.ma2a-part.meta.tmp")),
            written,
        )?;

        // When / Then: the next download starts instead of refusing its own file.
        assert_eq!(refusal(&destination).await, None, "{name}");
    }
    Ok(())
}
