//! The adapter's token file: one adapter at a time claims it, and its content is never
//! readable by another account, not even while it is being written.
//!
//! Everything the adapter keeps beside a token file is named `.ma2a-a2a-…`, a prefix no token
//! file may carry, in any ASCII letter case:
//!
//! - `.ma2a-a2a-locks/NAME` is the lock for the token file `NAME` in that directory. The lock
//!   carries the token file's own name, so the file system decides which spellings name one
//!   file, and every Runtime of this account, whatever its state directory, contends for the
//!   same lock.
//! - `.ma2a-a2a-stage-HEX/` is a fresh private directory the token is written in before it is
//!   renamed into place, so the file is private from the moment it exists.

use std::{
    ffi::OsStr,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use crate::AppError;

/// The prefix of every name the adapter keeps beside a token file.
const RESERVED_PREFIX: &str = ".ma2a-a2a-";
/// The private directory holding the locks of the token files beside it.
const LOCK_DIRECTORY: &str = ".ma2a-a2a-locks";
/// How many fresh staging names are tried before giving up.
const STAGING_ATTEMPTS: usize = 8;

/// The token file's directory, as an absolute path, and its name.
///
/// The directory keeps its given spelling rather than its canonical one, which on Windows
/// carries a `\\?\` prefix the ACL helper is not given.
fn locate(path: &Path) -> Result<(PathBuf, &OsStr), AppError> {
    let name = path
        .file_name()
        .ok_or(AppError::Usage("--token-file must name a file"))?;
    if is_reserved(name) {
        return Err(AppError::Usage(
            "--token-file cannot start with `.ma2a-a2a-`, which the adapter keeps for itself",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = std::path::absolute(parent)?;
    if std::fs::canonicalize(&parent)?
        .file_name()
        .is_some_and(is_reserved)
    {
        return Err(AppError::Usage(
            "--token-file cannot be inside a directory the adapter keeps for itself",
        ));
    }
    Ok((parent, name))
}

fn is_reserved(name: &OsStr) -> bool {
    name.as_encoded_bytes()
        .get(..RESERVED_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_PREFIX.as_bytes()))
}

fn store_error(error: ma2a_store::StoreError) -> AppError {
    AppError::Io(io::Error::other(error))
}

/// Claims the token file at `path` for as long as the returned lock file stays open.
///
/// A second adapter naming the same token file, by any spelling or from any state directory,
/// is refused here, before it writes anything, so a running adapter's published token file is
/// never replaced underneath its clients. The lock directory must be private to this account;
/// one that is not is refused and left as it was. The empty lock file stays behind: removing
/// it could let two claims race.
pub(super) fn claim(path: &Path) -> Result<std::fs::File, AppError> {
    let (parent, name) = locate(path)?;
    let locks = parent.join(LOCK_DIRECTORY);
    ma2a_store::ensure_private_directory(&locks, "A2A lock directory").map_err(store_error)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let lock = options.open(locks.join(name))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(std::fs::TryLockError::WouldBlock) => Err(AppError::Usage(
            "another adapter is serving with this token file; give this one its own --token-file",
        )),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Writes `token` to `path`, readable only by this user, replacing any earlier file.
pub(super) fn write_private(path: &Path, token: &str) -> Result<(), AppError> {
    write_staged(path, token, std::iter::repeat_with(random_name))
}

fn random_name() -> String {
    let mut bytes = [0_u8; 32];
    // A failed random source yields a fixed name, which `create_new` refuses when taken.
    let _filled = getrandom::fill(&mut bytes);
    let hex = blake3::Hash::from_bytes(bytes).to_hex();
    format!(
        "{RESERVED_PREFIX}stage-{}",
        hex.get(..32).unwrap_or_default()
    )
}

/// Writes `token` in a fresh private directory named by the first free name in `names`, then
/// renames it into place.
///
/// The directory is created owner-only (on Windows, with a protected ACL its children
/// inherit), so the token file inherits nothing from a shared parent directory, and no handle
/// to it can be opened by another account before it is protected. A name already taken, by
/// anything, is left alone and the next one tried. Only what this call created is removed.
fn write_staged(
    path: &Path,
    token: &str,
    names: impl IntoIterator<Item = String>,
) -> Result<(), AppError> {
    let (parent, name) = locate(path)?;
    let mut staging = None;
    for candidate in names.into_iter().take(STAGING_ATTEMPTS) {
        let directory = parent.join(candidate);
        match create_staging(&directory) {
            Ok(()) => {
                staging = Some(directory);
                break;
            }
            Err(ma2a_store::StoreError::Io(error))
                if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(store_error(error)),
        }
    }
    let staging = staging.ok_or_else(|| io::Error::other("no free staging name"))?;
    let staged = staging.join("token");
    let written = publish(&staged, &parent.join(name), token);
    if written.is_err() {
        let _removed = std::fs::remove_file(&staged);
    }
    let _removed = std::fs::remove_dir(&staging);
    written
}

/// Creates the staging directory `path`, which must not exist yet, private from the start.
///
/// On Windows a directory created plainly carries its parent's inherited entries until its ACL
/// is replaced, and a handle another account opens meanwhile keeps the access it was granted.
/// So it is created with a protected ACL for its owner and SYSTEM already in place, then given
/// the Store's exact current-user-and-SYSTEM ACL.
fn create_staging(path: &Path) -> Result<(), ma2a_store::StoreError> {
    #[cfg(windows)]
    {
        ma2a_services::windows_files::create_protected_dir(path)?;
        ma2a_store::protect_private_directory(path, "A2A token staging directory")
    }
    #[cfg(not(windows))]
    ma2a_store::create_private_directory(path)
}

fn publish(staged: &Path, destination: &Path, token: &str) -> Result<(), AppError> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(staged)?;
    ma2a_store::protect_private_file(staged, "A2A token").map_err(store_error)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    drop(file);
    Ok(std::fs::rename(staged, destination)?)
}

/// Removes `path` only while it still holds `token`: another adapter that wrote its own token
/// there since keeps it when this one stops.
pub(super) fn remove_if_ours(path: &Path, token: &str) {
    if std::fs::read(path).is_ok_and(|held| held == token.as_bytes()) {
        let _removed = std::fs::remove_file(path);
    }
}

#[cfg(test)]
#[path = "token_file_tests.rs"]
mod tests;
