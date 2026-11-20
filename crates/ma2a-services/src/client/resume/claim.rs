//! Which helper files a download may take over.
//!
//! Helper files live in a private directory under names derived from the destination, so an
//! ordinary file never carries one by accident. Whatever is placed there anyway is still not
//! trusted blindly: a download takes over only a sidecar, or a staged sidecar, that starts
//! with the MA2A header, a partial file next to such a sidecar or still empty, and an empty
//! lock file. Anything else is refused and left exactly as it was.

use std::{ffi::OsStr, io, io::Read as _};

use super::{
    SIDECAR_MAGIC,
    anchor::{Anchor, Open},
};

/// Refuses a sidecar, its staging file, or a partial file this download cannot show it wrote.
pub(super) fn ensure_ours(
    directory: &Anchor,
    (part, sidecar, staged): (&OsStr, &OsStr, &OsStr),
) -> io::Result<()> {
    // A staged sidecar is interrupted mid-write when a download stops before its rename, so any
    // prefix of the header, including none, is this download's own.
    has_header(directory, staged, true)?;
    let sidecar_ours = has_header(directory, sidecar, false)?;
    match directory.file_kind(part)? {
        Some((regular, len)) if !regular || (!sidecar_ours && len > 0) => Err(foreign(part)),
        _ => Ok(()),
    }
}

/// Returns whether `name` is a regular file that starts with the sidecar header, or with a
/// prefix of it when `partial`, `false` when it is absent, and refuses anything else.
fn has_header(directory: &Anchor, name: &OsStr, partial: bool) -> io::Result<bool> {
    match directory.file_kind(name)? {
        None => Ok(false),
        Some((false, _)) => Err(foreign(name)),
        Some((true, _)) => {
            let mut head = Vec::new();
            directory
                .open_file(name, Open::Read)?
                .take(u64::try_from(SIDECAR_MAGIC.len()).unwrap_or(u64::MAX))
                .read_to_end(&mut head)?;
            let magic = SIDECAR_MAGIC.as_bytes();
            if head == magic || (partial && magic.starts_with(&head)) {
                Ok(true)
            } else {
                Err(foreign(name))
            }
        }
    }
}

/// Refuses a lock file that is not empty: every lock file a download creates stays empty.
pub(super) fn ensure_lock_ours(file: &std::fs::File) -> io::Result<()> {
    if file.metadata()?.len() == 0 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "a lock file was not written by an MA2A download; move it away and retry",
        ))
    }
}

fn foreign(name: &OsStr) -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "`{}` was not written by an MA2A download; move it away and retry",
            name.to_string_lossy()
        ),
    )
}
