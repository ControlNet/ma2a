//! Resumable local download state: a partial file plus a sidecar naming what it continues.
//!
//! The sidecar is written durably before the first content byte, so a download interrupted
//! at any point, including by a daemon or machine restart, resumes only against the exact
//! source identity it started from. A lock file leases the destination to one download at
//! a time, so two downloads never write one partial file. All three live in a private
//! directory beside the destination (see [`partial_path`]), which the Files service never
//! lets a remote caller address, and every one of them is reached through directories held
//! open since the download started (see `helpers`).

use std::{
    ffi::{OsStr, OsString},
    io::{self, Read as _},
    path::Path,
};

use ma2a_core::service::{ContentIdentity, MAX_DATA_CHUNK_LEN};
use tokio::io::{AsyncSeekExt as _, AsyncWriteExt as _};

#[cfg(unix)]
#[path = "resume/anchor_unix.rs"]
mod anchor;
#[cfg(windows)]
#[path = "resume/anchor_windows.rs"]
mod anchor;
mod claim;
mod end;
mod helpers;
mod lease;
mod place;

use anchor::Open;
use helpers::Helpers;
use lease::Lease;
use place::suffixed;
pub use place::{PRIVATE_DIRECTORY, is_private_directory, partial_path};

const SIDECAR_MAGIC: &str = "ma2a-download v1";

/// One download in progress toward `destination`.
#[derive(Debug)]
pub struct Partial {
    /// The destination's file name, in the held destination directory.
    name: OsString,
    helpers: Helpers,
    lease: Lease,
    /// The partial file, its sidecar, and the sidecar's staging file, in the private directory.
    part: OsString,
    sidecar: OsString,
    staged: OsString,
    source: String,
    file: tokio::fs::File,
    offset: u64,
    identity: Option<ContentIdentity>,
    hasher: blake3::Hasher,
}

impl Partial {
    /// Opens the partial download of `source` into `destination`.
    ///
    /// With `resume`, a partial file whose sidecar names the same source continues from its
    /// length; anything else starts over from an empty partial file.
    ///
    /// # Errors
    /// Returns [`io::ErrorKind::WouldBlock`] while another download holds `destination`, or
    /// another local file error.
    pub async fn open(destination: &Path, source: &str, resume: bool) -> io::Result<Self> {
        let destination = destination.to_path_buf();
        let source = source.to_owned();
        tokio::task::spawn_blocking(move || Self::open_blocking(&destination, source, resume))
            .await
            .map_err(io::Error::other)?
    }

    fn open_blocking(destination: &Path, source: String, resume: bool) -> io::Result<Self> {
        let name = place::file_name(destination)?.to_os_string();
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let (helpers, lease) = Helpers::open(parent, &name)?;
        let part = place::partial_name(&name);
        let sidecar = suffixed(&part, ".meta");
        let staged = suffixed(&sidecar, ".tmp");
        if let Err(error) = claim::ensure_ours(&helpers.directory, (&part, &sidecar, &staged)) {
            // The lock file was checked to be this download's own, so it goes too.
            lease.release(&helpers.locks);
            helpers.tidy();
            return Err(error);
        }
        let continued = if resume {
            continued(&helpers.directory, (&part, &sidecar), &source)?
        } else {
            None
        };
        let (file, offset, identity, hasher) = if let Some(continued) = continued {
            continued
        } else {
            // Empty the partial file before forgetting its sidecar, so an interruption between
            // the two leaves helper files a later download still recognizes as its own.
            let file = helpers.directory.open_file(&part, Open::Truncate)?;
            remove_if_present(&helpers.directory, &sidecar)?;
            (file, 0, None, blake3::Hasher::new())
        };
        Ok(Self {
            name,
            helpers,
            lease,
            part,
            sidecar,
            staged,
            source,
            file: tokio::fs::File::from_std(file),
            offset,
            identity,
            hasher,
        })
    }

    /// Returns the content offset to request.
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the identity a resumed download must name.
    pub const fn identity(&self) -> Option<ContentIdentity> {
        self.identity
    }

    /// Flushes any pending asynchronous writes to the underlying partial file.
    ///
    /// # Errors
    /// Returns a local file error.
    pub async fn flush(&mut self) -> io::Result<()> {
        self.file.flush().await
    }

    /// Returns whether the partial file already holds at least one written byte.
    pub async fn has_bytes(&mut self) -> bool {
        let _flushed = self.file.flush().await;
        self.file
            .metadata()
            .await
            .is_ok_and(|metadata| metadata.len() > 0)
    }

    /// Records the identity the target is about to stream, before any content arrives.
    ///
    /// # Errors
    /// Returns a local file error.
    pub async fn begin(&mut self, identity: ContentIdentity) -> io::Result<()> {
        if self.identity == Some(identity) {
            return Ok(());
        }
        let text = format!(
            "{SIDECAR_MAGIC}\n{}\n{}\n{}\n",
            self.source,
            identity.size(),
            identity.modified_ns()
        );
        let directory = &self.helpers.directory;
        let mut file =
            tokio::fs::File::from_std(directory.open_file(&self.staged, Open::Truncate)?);
        file.write_all(text.as_bytes()).await?;
        file.sync_all().await?;
        drop(file);
        directory.rename(&self.staged, (directory, &self.sidecar))?;
        self.identity = Some(identity);
        Ok(())
    }

    /// Lends the partial file and the hash of everything in it for the content stream.
    pub const fn sink(&mut self) -> (&mut tokio::fs::File, &mut blake3::Hasher) {
        (&mut self.file, &mut self.hasher)
    }

    /// Empties the partial file and forgets its sidecar while keeping the destination's lease,
    /// for a source that changed since the partial download began.
    ///
    /// # Errors
    /// Returns a local file error.
    pub async fn restart(&mut self) -> io::Result<()> {
        // The very file held since the download began is emptied, before its sidecar is
        // forgotten, as in `open`.
        self.file.set_len(0).await?;
        self.file.seek(io::SeekFrom::Start(0)).await?;
        remove_if_present(&self.helpers.directory, &self.sidecar)?;
        self.offset = 0;
        self.identity = None;
        self.hasher = blake3::Hasher::new();
        Ok(())
    }
}

type Continued = (std::fs::File, u64, Option<ContentIdentity>, blake3::Hasher);

/// Reopens a partial file whose sidecar names `source`, hashing the prefix it already holds.
fn continued(
    directory: &anchor::Anchor,
    (part, sidecar): (&OsStr, &OsStr),
    source: &str,
) -> io::Result<Option<Continued>> {
    let mut text = String::new();
    match directory.open_file(sidecar, Open::Read) {
        Ok(mut file) => drop(file.read_to_string(&mut text)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut lines = text.lines();
    let (Some(SIDECAR_MAGIC), Some(named), Some(size), Some(modified), None) = (
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
        lines.next(),
    ) else {
        return Ok(None);
    };
    let (Ok(size), Ok(modified)) = (size.parse::<u64>(), modified.parse::<u64>()) else {
        return Ok(None);
    };
    let mut file = match directory.open_file(part, Open::Existing) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Reading to the end leaves the file positioned there, where the content continues.
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    let mut offset = 0_u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(buffer.get(..read).unwrap_or_default());
        offset = offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
    }
    if named != source || offset > size {
        return Ok(None);
    }
    let identity = ContentIdentity::new(size, modified);
    Ok(Some((file, offset, Some(identity), hasher)))
}

fn remove_if_present(directory: &anchor::Anchor, name: &OsStr) -> io::Result<()> {
    match directory.remove_file(name) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
