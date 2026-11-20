//! Where a download keeps its helper files: a private directory beside the destination.
//!
//! Helper names are derived from the destination's name rather than appended to it, so no
//! helper file shares a name with an ordinary file in the destination's directory, nor with
//! another download's destination or helper file there. The directory is created owner-only
//! on Unix and removed again once the last download in it ends.

use std::{
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
};

/// The private directory, beside each destination, that holds its download's helper files.
pub const PRIVATE_DIRECTORY: &str = ".ma2a-downloads";

/// Returns where the partial file of a download to `destination` lives.
///
/// Its sidecar, staged sidecar, and lock file are the same name with `.meta`, `.meta.tmp`,
/// and `.lock` appended. Names differing only in ASCII case map to one partial file, so a
/// destination a case-insensitive file system names twice is still leased once.
///
/// # Errors
/// Returns [`io::ErrorKind::InvalidInput`] when `destination` has no file name.
pub fn partial_path(destination: &Path) -> io::Result<PathBuf> {
    let name = destination
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let digest = blake3::hash(&name.as_encoded_bytes().to_ascii_lowercase());
    let mut stem = String::with_capacity(48);
    for byte in digest.as_bytes().iter().take(16) {
        let _written = write!(stem, "{byte:02x}");
    }
    stem.push_str(".ma2a-part");
    Ok(destination.with_file_name(PRIVATE_DIRECTORY).join(stem))
}

/// Makes sure the private directory holding `helper` exists, and refuses anything else that
/// carries its name: a file, or a link, is left exactly as it was.
pub(super) fn prepare(helper: &Path) -> io::Result<()> {
    let directory = helper
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    for _ in 0..2 {
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => return Err(foreign_directory()),
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            Err(_) => {}
        }
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(directory) {
            // Another download may have created it meanwhile; look again.
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
    }
    Err(foreign_directory())
}

/// Removes the private directory holding `helper` once no download keeps anything in it.
pub(super) async fn tidy(helper: &Path) {
    if let Some(directory) = helper.parent() {
        // Fails, and keeps the directory, while any other download still holds its lock there.
        let _removed = tokio::fs::remove_dir(directory).await;
    }
}

fn foreign_directory() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "`{PRIVATE_DIRECTORY}` is not a directory MA2A downloads can use; move it away and retry"
        ),
    )
}

/// Returns `path` with `suffix` appended to its file name.
pub(super) fn suffixed(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let mut name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}
