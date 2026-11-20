//! Which helper files a download may take over.
//!
//! A helper name is the destination's name plus a fixed suffix, so an unrelated file can carry
//! it: another download's finished file, or anything the operator keeps there. A download
//! takes over only what it can show it wrote: a sidecar that starts with the MA2A header, a
//! partial file next to such a sidecar or still empty, and an empty lock file. Anything else is
//! refused and left exactly as it was.

use std::{io, path::Path};

use tokio::io::AsyncReadExt as _;

use super::SIDECAR_MAGIC;

/// Refuses a sidecar or partial file this download cannot show it wrote.
pub(super) async fn ensure_ours(part: &Path, sidecar: &Path) -> io::Result<()> {
    let sidecar_ours = match tokio::fs::symlink_metadata(sidecar).await {
        Ok(metadata) if !metadata.is_file() => return Err(foreign(sidecar)),
        Ok(_) => {
            let mut head = Vec::new();
            tokio::fs::File::open(sidecar)
                .await?
                .take(u64::try_from(SIDECAR_MAGIC.len()).unwrap_or(u64::MAX))
                .read_to_end(&mut head)
                .await?;
            if head != SIDECAR_MAGIC.as_bytes() {
                return Err(foreign(sidecar));
            }
            true
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    match tokio::fs::symlink_metadata(part).await {
        Ok(metadata) if !metadata.is_file() || (!sidecar_ours && metadata.len() > 0) => {
            Err(foreign(part))
        }
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
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
