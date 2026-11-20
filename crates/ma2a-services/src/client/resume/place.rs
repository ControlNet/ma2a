//! What a download's helper files are called, inside the private directory beside the
//! destination.
//!
//! - `HASH.ma2a-part`, with `.meta` and `.meta.tmp`: the partial file, its sidecar, and the
//!   sidecar's staging file. `HASH` follows the destination's exact name, so no helper file
//!   shares a name with another download's, and two destinations a case-sensitive file system
//!   keeps apart keep their own progress.
//! - `locks/NAME`: the lock file of the destination `NAME`. It carries the destination's own
//!   name, so the file system decides which spellings name one destination, and each has a
//!   single writer whatever the volume's name rules.
//! - `created`: present only in a private directory a download made, which a download may
//!   therefore remove once nothing else is left in it.

use std::{
    ffi::{OsStr, OsString},
    fmt::Write as _,
    io,
    path::{Path, PathBuf},
};

/// The private directory, beside each destination, that holds its download's helper files.
pub const PRIVATE_DIRECTORY: &str = ".ma2a-downloads";
/// The directory, inside the private one, holding the lock files.
pub(super) const LOCKS: &str = "locks";
/// The marker of a private directory a download created.
pub(super) const CREATED: &str = "created";

/// Returns where the partial file of a download to `destination` lives.
///
/// # Errors
/// Returns [`io::ErrorKind::InvalidInput`] when `destination` has no file name or is named
/// like the private directory itself.
pub fn partial_path(destination: &Path) -> io::Result<PathBuf> {
    let name = file_name(destination)?;
    Ok(destination
        .with_file_name(PRIVATE_DIRECTORY)
        .join(partial_name(name)))
}

/// The partial file's name for the destination `name`: 128 bits of BLAKE3 of its exact bytes.
pub(super) fn partial_name(name: &OsStr) -> OsString {
    let digest = blake3::hash(name.as_encoded_bytes());
    let mut stem = String::with_capacity(48);
    for byte in digest.as_bytes().iter().take(16) {
        let _written = write!(stem, "{byte:02x}");
    }
    stem.push_str(".ma2a-part");
    stem.into()
}

/// Whether `name` is the private directory's, in any ASCII case.
pub fn is_private_directory(name: &OsStr) -> bool {
    name.as_encoded_bytes()
        .eq_ignore_ascii_case(PRIVATE_DIRECTORY.as_bytes())
}

/// Returns the destination's file name, refusing one named like the private directory.
pub(super) fn file_name(destination: &Path) -> io::Result<&OsStr> {
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

/// Returns `name` with `suffix` appended.
pub(super) fn suffixed(name: &OsStr, suffix: &str) -> OsString {
    let mut name = name.to_os_string();
    name.push(suffix);
    name
}

pub(super) fn foreign_directory() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "`{PRIVATE_DIRECTORY}` is not a private directory this account owns, so MA2A downloads cannot use it; move it away and retry"
        ),
    )
}
