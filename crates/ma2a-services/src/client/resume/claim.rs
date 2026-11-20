//! Which helper files a download may take over.
//!
//! A helper name is the destination's name plus a fixed suffix, so an unrelated file can carry
//! it: another download's finished file, or anything the operator keeps there. A download
//! takes over only what it can show it wrote: a sidecar, or a staged sidecar, that starts with
//! the MA2A header, a partial file next to such a sidecar or still empty, and an empty lock file. Anything else is
//! refused and left exactly as it was.

use std::{io, path::Path};

use tokio::io::AsyncReadExt as _;

use super::SIDECAR_MAGIC;

/// Refuses a sidecar, its staging file, or a partial file this download cannot show it wrote.
pub(super) async fn ensure_ours(part: &Path, sidecar: &Path, staged: &Path) -> io::Result<()> {
    has_header(staged).await?;
    let sidecar_ours = has_header(sidecar).await?;
    match tokio::fs::symlink_metadata(part).await {
        Ok(metadata) if !metadata.is_file() || (!sidecar_ours && metadata.len() > 0) => {
            Err(foreign(part))
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Returns whether `path` is a regular file that starts with the sidecar header, `false` when
/// it is absent, and refuses anything else.
async fn has_header(path: &Path) -> io::Result<bool> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata.is_file() => Err(foreign(path)),
        Ok(_) => {
            let mut head = Vec::new();
            tokio::fs::File::open(path)
                .await?
                .take(u64::try_from(SIDECAR_MAGIC.len()).unwrap_or(u64::MAX))
                .read_to_end(&mut head)
                .await?;
            if head == SIDECAR_MAGIC.as_bytes() {
                Ok(true)
            } else {
                Err(foreign(path))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Refuses a lock file that is not an empty regular file: every lock file a download creates
/// stays empty.
pub(super) fn ensure_lock_ours(file: &std::fs::File, path: &Path) -> io::Result<()> {
    let named = std::fs::symlink_metadata(path)?;
    if named.is_file() && file.metadata()?.len() == 0 {
        Ok(())
    } else {
        Err(foreign(path))
    }
}

fn foreign(path: &Path) -> io::Error {
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("`{name}` was not written by an MA2A download; move it away and retry"),
    )
}
