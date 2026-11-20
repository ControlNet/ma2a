//! How a download ends: its verified partial file is published, or everything is discarded.

use std::io;

use tokio::io::AsyncWriteExt as _;

use super::{Partial, remove_if_present};

impl Partial {
    /// Publishes the verified partial file as the destination and forgets the sidecar.
    ///
    /// The partial file is renamed from the private directory held since the download began
    /// into the destination's directory held since then too, so it is the very file whose
    /// content was hashed that is published, next to nothing but the destination's own name.
    /// Without `replace` the rename never replaces an existing file. On Unix, publishing first
    /// checks that the destination's path still names the held directory and that the partial
    /// file's name still refers to the hashed file; a mismatch fails the download and keeps its
    /// progress for a later resume.
    ///
    /// # Errors
    /// Returns [`io::ErrorKind::AlreadyExists`] when the destination exists and `replace` is
    /// false (the partial file is kept), or another local file error.
    pub async fn finish(mut self, replace: bool) -> io::Result<()> {
        self.file.flush().await?;
        self.file.sync_all().await?;
        let Self {
            name,
            helpers,
            lease,
            part,
            sidecar,
            staged,
            file,
            ..
        } = self;
        let file = file.into_std().await;
        tokio::task::spawn_blocking(move || {
            #[cfg(unix)]
            if !helpers.parent.0.is_at(&helpers.parent.1)? {
                lease.release(&helpers.locks);
                return Err(moved());
            }
            #[cfg(unix)]
            if !helpers.directory.names_file(&part, &file)? {
                lease.release(&helpers.locks);
                return Err(replaced());
            }
            // An open file without delete sharing cannot be renamed on Windows.
            #[cfg(windows)]
            drop(file);
            let to = (&helpers.parent.0, name.as_os_str());
            let published = if replace {
                helpers.directory.rename(&part, to)
            } else {
                helpers.directory.rename_no_clobber(&part, to)
            };
            if let Err(error) = published {
                lease.release(&helpers.locks);
                return Err(error);
            }
            #[cfg(unix)]
            if !helpers.parent.0.names_file(&name, &file)? {
                // Whatever is there now may not be ours to remove; report it instead.
                lease.release(&helpers.locks);
                return Err(replaced());
            }
            remove_if_present(&helpers.directory, &sidecar)?;
            remove_if_present(&helpers.directory, &staged)?;
            lease.release(&helpers.locks);
            helpers.tidy();
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Removes the partial file and its sidecar, for a source that no longer matches.
    ///
    /// # Errors
    /// Returns a local file error.
    pub async fn discard(self) -> io::Result<()> {
        let Self {
            helpers,
            lease,
            part,
            sidecar,
            staged,
            file,
            ..
        } = self;
        drop(file.into_std().await);
        tokio::task::spawn_blocking(move || {
            for name in [&part, &sidecar, &staged] {
                remove_if_present(&helpers.directory, name)?;
            }
            lease.release(&helpers.locks);
            helpers.tidy();
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }
}

#[cfg(unix)]
fn moved() -> io::Error {
    io::Error::other(
        "the destination's directory was moved or replaced during the download; its progress is kept for --resume",
    )
}

#[cfg(unix)]
fn replaced() -> io::Error {
    io::Error::other("a helper file of this download was replaced; nothing was published")
}
