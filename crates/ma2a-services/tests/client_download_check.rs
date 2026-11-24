//! A download opened with a check touches no helper file before the check passes.

#[path = "support/host.rs"]
mod support;

use std::{
    io,
    path::{Path, PathBuf},
};

use ma2a_core::service::ContentIdentity;
use ma2a_services::client::resume::{PRIVATE_DIRECTORY, Partial};
use support::{TempRoot, TestResult};
use tokio::io::AsyncWriteExt as _;

/// Every file below `root` with its content, in path order.
fn snapshot(root: &Path) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let content = std::fs::read(&path)?;
                found.push((
                    path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    content,
                ));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// Leaves a resumable partial download of `destination` holding a prefix.
async fn leave_progress(destination: &Path) -> TestResult {
    let mut partial = Partial::open(destination, "peer docs:/file", true).await?;
    partial.begin(ContentIdentity::new(10, 7)).await?;
    let (file, hasher) = partial.sink();
    file.write_all(b"prefix").await?;
    hasher.update(b"prefix");
    file.flush().await?;
    drop(partial);
    Ok(())
}

#[tokio::test]
async fn a_failed_check_leaves_the_progress_of_an_earlier_download_as_it_was() -> TestResult {
    // Given
    let local = TempRoot::new("client-download-check")?;
    let destination = local.path().join("file");
    leave_progress(&destination).await?;
    let before = snapshot(&local.path().join(PRIVATE_DIRECTORY))?;
    assert!(!before.is_empty());

    // When
    let opened = Partial::open_checked(&destination, ("peer docs:/file", false), || {
        Err(io::Error::new(io::ErrorKind::InvalidInput, "refused"))
    })
    .await;

    // Then
    assert_eq!(
        opened.map(|_| ()).map_err(|error| error.kind()),
        Err(io::ErrorKind::InvalidInput)
    );
    assert_eq!(snapshot(&local.path().join(PRIVATE_DIRECTORY))?, before);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn a_directory_swapped_while_it_is_opened_is_left_as_it_was() -> TestResult {
    // Given: progress in `dir`, which the check replaces with a new directory of that name.
    let local = TempRoot::new("client-download-swap")?;
    let held = local.path().join("dir");
    std::fs::create_dir(&held)?;
    leave_progress(&held.join("file")).await?;
    let before = snapshot(&held.join(PRIVATE_DIRECTORY))?;
    let (named, moved) = (held.clone(), local.path().join("old"));

    // When
    let opened = Partial::open_checked(&held.join("file"), ("peer docs:/file", false), move || {
        std::fs::rename(&named, &moved)?;
        std::fs::create_dir(&named)
    })
    .await;

    // Then: neither the held directory nor the new one is touched.
    assert!(opened.is_err());
    assert_eq!(
        snapshot(&local.path().join("old").join(PRIVATE_DIRECTORY))?,
        before
    );
    assert!(!held.join(PRIVATE_DIRECTORY).exists());
    Ok(())
}
