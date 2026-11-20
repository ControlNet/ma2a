//! Where a download keeps its helper files: a private directory beside the destination.
//!
//! Helper names are derived from the destination's name rather than appended to it, so no
//! helper file shares a name with an ordinary file in the destination's directory, nor with
//! another download's destination or helper file there. The directory is created owner-only
//! on Unix, an existing one is used only when this account owns it and nobody else may enter
//! it, and it is removed again once the last download in it ends.

use std::{
    ffi::OsStr,
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
};

/// The private directory, beside each destination, that holds its download's helper files.
pub const PRIVATE_DIRECTORY: &str = ".ma2a-downloads";

/// Returns where the partial file of a download to `destination` lives.
///
/// Its sidecar and staged sidecar are the same name with `.meta` and `.meta.tmp` appended.
/// The name follows the destination's exact bytes, so two destinations a case-sensitive file
/// system keeps apart never share, or clear, each other's progress.
///
/// # Errors
/// Returns [`io::ErrorKind::InvalidInput`] when `destination` has no file name or is named
/// like the private directory itself.
pub fn partial_path(destination: &Path) -> io::Result<PathBuf> {
    let name = file_name(destination)?;
    Ok(helper(destination, name.as_encoded_bytes(), ".ma2a-part"))
}

/// Returns the lock file a download to `destination` holds.
///
/// Unlike the partial file, it follows the name up to ASCII case: a case-insensitive file
/// system names one destination by every spelling, and it must still have a single writer.
/// Two destinations differing only in case on a case-sensitive file system therefore wait for
/// each other, which is safe.
pub(super) fn lock_path(destination: &Path) -> io::Result<PathBuf> {
    let name = file_name(destination)?;
    let folded = name.as_encoded_bytes().to_ascii_lowercase();
    Ok(helper(destination, &folded, ".ma2a-lock"))
}

/// Whether `name` is the private directory's, in any ASCII case.
pub fn is_private_directory(name: &OsStr) -> bool {
    name.as_encoded_bytes()
        .eq_ignore_ascii_case(PRIVATE_DIRECTORY.as_bytes())
}

fn file_name(destination: &Path) -> io::Result<&OsStr> {
    let name = destination
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    if is_private_directory(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{PRIVATE_DIRECTORY}` is reserved for MA2A downloads; choose another name"),
        ));
    }
    Ok(name)
}

/// The helper file named by 128 bits of BLAKE3 of `key`, with `suffix`.
fn helper(destination: &Path, key: &[u8], suffix: &str) -> PathBuf {
    let digest = blake3::hash(key);
    let mut stem = String::with_capacity(48);
    for byte in digest.as_bytes().iter().take(16) {
        let _written = write!(stem, "{byte:02x}");
    }
    stem.push_str(suffix);
    destination.with_file_name(PRIVATE_DIRECTORY).join(stem)
}

/// Makes sure the private directory holding `helper` exists, and refuses anything else that
/// carries its name: a file, or a link, is left exactly as it was.
pub(super) fn prepare(helper: &Path) -> io::Result<()> {
    let directory = helper
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    for _ in 0..2 {
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() => return private(&metadata),
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

/// Accepts an existing directory only when this account owns it and no other account may
/// list or enter it. Windows directories inherit the parent's ACL, as the destination does.
fn private(metadata: &std::fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(foreign_directory());
        }
    }
    #[cfg(not(unix))]
    let _unchecked = metadata;
    Ok(())
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
            "`{PRIVATE_DIRECTORY}` is not a private directory this account owns, so MA2A downloads cannot use it; move it away and retry"
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
