//! Resumable local download state: a partial file plus a sidecar naming what it continues.
//!
//! The sidecar is written durably before the first content byte, so a download interrupted
//! at any point, including by a daemon or machine restart, resumes only against the exact
//! source identity it started from. A lock file leases the destination to one download at
//! a time, so two downloads never write one partial file.

use std::{
    io,
    path::{Path, PathBuf},
};

use ma2a_core::service::{ContentIdentity, MAX_DATA_CHUNK_LEN};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

mod claim;
mod lease;

use lease::Lease;

const SIDECAR_MAGIC: &str = "ma2a-download v1";

/// One download in progress toward `destination`.
#[derive(Debug)]
pub struct Partial {
    destination: PathBuf,
    part: PathBuf,
    sidecar: PathBuf,
    lease: Lease,
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
        let lease = Lease::acquire(suffixed(destination, ".ma2a-part.lock")?).await?;
        let part = suffixed(destination, ".ma2a-part")?;
        let sidecar = suffixed(destination, ".ma2a-part.meta")?;
        if let Err(error) = claim::ensure_ours(&part, &sidecar, &suffixed(&sidecar, ".tmp")?).await
        {
            // The lock file was checked to be this download's own, so it goes too.
            lease.release().await;
            return Err(error);
        }
        let continued = if resume {
            continued(&part, &sidecar, source).await?
        } else {
            None
        };
        let (file, offset, identity, hasher) = if let Some(continued) = continued {
            continued
        } else {
            // Empty the partial file before forgetting its sidecar, so an interruption between
            // the two leaves helper files a later download still recognizes as its own.
            let file = tokio::fs::File::create(&part).await?;
            remove_if_present(&sidecar).await?;
            (file, 0, None, blake3::Hasher::new())
        };
        Ok(Self {
            destination: destination.to_path_buf(),
            part,
            sidecar,
            lease,
            source: source.to_owned(),
            file,
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
        let staged = suffixed(&self.sidecar, ".tmp")?;
        let mut file = tokio::fs::File::create(&staged).await?;
        file.write_all(text.as_bytes()).await?;
        file.sync_all().await?;
        tokio::fs::rename(&staged, &self.sidecar).await?;
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
        // A resumed partial file is open for appending, which may not allow truncation. It is
        // emptied before its sidecar is forgotten, as in `open`.
        self.file = tokio::fs::File::create(&self.part).await?;
        remove_if_present(&self.sidecar).await?;
        self.offset = 0;
        self.identity = None;
        self.hasher = blake3::Hasher::new();
        Ok(())
    }

    /// Publishes the verified partial file as the destination and forgets the sidecar.
    ///
    /// Without `replace`, the destination is created by a hard link, which fails rather than
    /// replace a file that appeared meanwhile; a file system without hard links falls back to
    /// a check before the rename.
    ///
    /// # Errors
    /// Returns [`io::ErrorKind::AlreadyExists`] when the destination exists and `replace` is
    /// false (the partial file is kept), or another local file error.
    pub async fn finish(mut self, replace: bool) -> io::Result<()> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        drop(self.file);
        if replace {
            tokio::fs::rename(&self.part, &self.destination).await?;
        } else {
            match tokio::fs::hard_link(&self.part, &self.destination).await {
                Ok(()) => tokio::fs::remove_file(&self.part).await?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return Err(error),
                Err(_) if tokio::fs::try_exists(&self.destination).await? => {
                    return Err(io::ErrorKind::AlreadyExists.into());
                }
                Err(_) => tokio::fs::rename(&self.part, &self.destination).await?,
            }
        }
        remove_if_present(&self.sidecar).await?;
        self.lease.release().await;
        Ok(())
    }

    /// Removes the partial file and its sidecar, for a source that no longer matches.
    ///
    /// # Errors
    /// Returns a local file error.
    pub async fn discard(self) -> io::Result<()> {
        drop(self.file);
        remove_if_present(&self.part).await?;
        remove_if_present(&self.sidecar).await?;
        self.lease.release().await;
        Ok(())
    }
}

type Continued = (
    tokio::fs::File,
    u64,
    Option<ContentIdentity>,
    blake3::Hasher,
);

/// Reopens a partial file whose sidecar names `source`, hashing the prefix it already holds.
async fn continued(part: &Path, sidecar: &Path, source: &str) -> io::Result<Option<Continued>> {
    let text = match tokio::fs::read_to_string(sidecar).await {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
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
    let mut file = match tokio::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(part)
        .await
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; MAX_DATA_CHUNK_LEN];
    let mut offset = 0_u64;
    loop {
        let read = file.read(&mut buffer).await?;
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

fn suffixed(path: &Path, suffix: &str) -> io::Result<PathBuf> {
    let mut name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}

async fn remove_if_present(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
