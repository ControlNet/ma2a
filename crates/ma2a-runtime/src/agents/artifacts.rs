//! Artifacts: immutable, content-addressed blobs served through the Transfer download path.
//!
//! A blob is named by its BLAKE3, so identical content is stored once and a resumed
//! download can never observe changed content. Blobs no retained task references are
//! removed once they are old enough that no publication can still be recording them.

use std::{
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use ma2a_core::service::{
    AgentTaskId, ArtifactRef, ContentIdentity, RemotePath, RootName, ServiceError,
};
use ma2a_services::{FrameReader, FrameWriter, OperationEnd, OperationReport};
use ma2a_store::AgentTaskChange;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

use super::{AgentManager, convert::hex, host::refuse, private_dir, tasks::Viewer};

/// Largest artifact: 256 MiB.
pub(super) const MAX_ARTIFACT_SIZE: u64 = 256 * 1_048_576;
/// Age after which an unreferenced blob is removed.
const ORPHAN_AGE: Duration = Duration::from_secs(600);

/// One artifact request: who asks, which artifact, and where a resumed download starts.
type ArtifactRequest<'a> = (
    Viewer<'a>,
    (AgentTaskId, String),
    (u64, Option<ContentIdentity>),
);

/// Opens a regular file for reading without ever blocking on a FIFO swapped in after its
/// path was checked.
pub(super) fn open_regular(path: &Path) -> io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed());
    }
    let file = options.open(path)?;
    if file.metadata()?.is_file() {
        Ok(file)
    } else {
        Err(io::Error::from(io::ErrorKind::InvalidInput))
    }
}

/// Copies the open regular file `input` into a staging file of the blob store; returns its
/// size, its hash, and the staging file.
fn stage_blob(mut input: std::fs::File, blobs: &Path) -> io::Result<(u64, [u8; 32], PathBuf)> {
    private_dir(blobs)?;
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(io::Error::other)?;
    let staging = blobs.join(format!(".staging-{}", hex(&suffix)));
    let copied = (|| {
        let mut output = std::fs::File::create_new(&staging)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0; 65_536];
        let mut size = 0_u64;
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            size += u64::try_from(read).unwrap_or(u64::MAX);
            if size > MAX_ARTIFACT_SIZE {
                return Err(io::Error::from(io::ErrorKind::FileTooLarge));
            }
            let chunk = buffer.get(..read).unwrap_or_default();
            hasher.update(chunk);
            output.write_all(chunk)?;
        }
        output.sync_all()?;
        Ok((size, *hasher.finalize().as_bytes()))
    })();
    match copied {
        Ok((size, hash)) => Ok((size, hash, staging)),
        Err(error) => {
            let _removed = std::fs::remove_file(&staging);
            Err(error)
        }
    }
}

/// Whether `blob` still holds exactly the content of `size` bytes named `hash`.
fn holds(blob: &Path, (size, hash): (u64, &[u8; 32])) -> io::Result<bool> {
    let mut file = open_regular(blob)?;
    if file.metadata()?.len() != size {
        return Ok(false);
    }
    let mut hasher = blake3::Hasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().as_bytes() == hash)
}

/// Makes a staged copy the blob of `hash`. A sound blob already stored is left untouched, so
/// a resumed download of it keeps its content identity, and the copy is discarded; a damaged
/// one is replaced by the copy.
fn place_blob(staging: &Path, blobs: &Path, (size, hash): (u64, &[u8; 32])) -> io::Result<()> {
    let blob = blobs.join(hex(hash));
    let placed = if blob.try_exists()? && holds(&blob, (size, hash))? {
        std::fs::remove_file(staging)
    } else {
        std::fs::rename(staging, &blob)
    };
    if placed.is_err() {
        let _removed = std::fs::remove_file(staging);
        return placed;
    }
    // The rename must survive a crash before a task commits a reference to the blob, and so
    // must `blobs/` itself, which the first publication creates.
    #[cfg(unix)]
    {
        std::fs::File::open(blobs)?.sync_all()?;
        if let Some(parent) = blobs.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

impl AgentManager {
    /// Publishes one workspace file as an artifact of a live task for the tool call `call`.
    ///
    /// The artifact and the call's answer commit together, so a restart never runs the same
    /// publication again and finds its own name taken.
    pub(super) async fn publish(
        &self,
        (task, call): (AgentTaskId, &str),
        (name, source, media_type): (&str, std::fs::File, &str),
    ) -> Result<ArtifactRef, ServiceError> {
        let blobs = self.blobs();
        let staged = tokio::task::spawn_blocking(move || stage_blob(source, &blobs))
            .await
            .map_err(|_| ServiceError::Internal)?;
        let (size, hash, staging) = staged.map_err(|error| match error.kind() {
            io::ErrorKind::FileTooLarge => ServiceError::TooLarge,
            io::ErrorKind::NotFound => ServiceError::NotFound,
            _ => ServiceError::InvalidInput,
        })?;
        let artifact = ArtifactRef::new((name, media_type), (size, hash));
        let Ok(artifact) = artifact else {
            let _removed = tokio::fs::remove_file(&staging).await;
            return Err(ServiceError::InvalidInput);
        };
        // Collection waits until the blob is placed and referenced, so a blob kept as
        // already stored cannot be collected before this task references it.
        let _placing = self.0.blob_lock.lock().await;
        let blobs = self.blobs();
        tokio::task::spawn_blocking(move || place_blob(&staging, &blobs, (size, &hash)))
            .await
            .map_err(|_| ServiceError::Internal)?
            .map_err(|_| ServiceError::Internal)?;
        let (recorded, at_ms) = (artifact.clone(), self.now_ms());
        let (call, answer) = (call.to_owned(), super::tools::published(&artifact));
        match self
            .commit(move |repository| {
                repository.publish_agent_artifact((task, (&call, &answer)), (&recorded, at_ms))
            })
            .await?
        {
            AgentTaskChange::Changed(_) => Ok(artifact),
            AgentTaskChange::Ended(_) => Err(ServiceError::TaskEnded),
            AgentTaskChange::Refused => Err(ServiceError::Conflict),
            AgentTaskChange::Missing => Err(ServiceError::UnknownTask),
        }
    }

    /// Serves one artifact with exactly the Transfer v1 download frames.
    pub(super) async fn serve_artifact<R, W>(
        &self,
        ((viewer, (task, name), (offset, identity)), stop): (
            ArtifactRequest<'_>,
            &CancellationToken,
        ),
        (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    ) -> OperationReport
    where
        R: AsyncRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
        if stop.is_cancelled() {
            return OperationReport::new(OperationEnd::Failed(ServiceError::Unavailable));
        }
        let details = match self.details(viewer, task).await {
            Ok(details) => details,
            Err(error) => return refuse(writer, error).await,
        };
        let Some(artifact) = details.artifacts.iter().find(|found| found.name() == name) else {
            return refuse(writer, ServiceError::UnknownArtifact).await;
        };
        let path = RootName::new("artifacts")
            .and_then(|root| RemotePath::new(root, vec![hex(artifact.blake3())]));
        let Ok(path) = path else {
            return refuse(writer, ServiceError::Internal).await;
        };
        // The blob must still be the content recorded for the task, not merely whatever bytes
        // its file holds now.
        let content = (artifact.size(), *artifact.blake3());
        tokio::select! {
            biased;
            () = stop.cancelled() => OperationReport::new(OperationEnd::Failed(ServiceError::Unavailable)),
            report = ma2a_services::serve_blob(
                self.blobs(),
                ((path, offset, identity), content),
                (reader, writer),
            ) => report,
        }
    }

    /// Removes every old blob no retained task references.
    pub(super) async fn collect_blobs(&self) {
        let _collecting = self.0.blob_lock.lock().await;
        let Ok(referenced) = self
            .store(|repository| repository.referenced_artifact_hashes())
            .await
        else {
            return;
        };
        let referenced: std::collections::HashSet<String> =
            referenced.iter().map(|hash| hex(hash)).collect();
        let blobs = self.blobs();
        let _collected = tokio::task::spawn_blocking(move || -> io::Result<()> {
            let now = SystemTime::now();
            for entry in std::fs::read_dir(&blobs)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let age = entry
                    .metadata()?
                    .modified()
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok());
                if !referenced.contains(&name) && age.is_some_and(|age| age >= ORPHAN_AGE) {
                    let _removed = std::fs::remove_file(entry.path());
                }
            }
            Ok(())
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{hex, open_regular, place_blob, stage_blob};

    #[test]
    fn publishing_stored_content_again_keeps_the_blob_untouched()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let dir = std::env::temp_dir().join(format!("ma2a-blobs-{}", hex(&nonce)));
        std::fs::create_dir_all(&dir)?;
        let (source, blobs) = (dir.join("report.txt"), dir.join("blobs"));
        std::fs::write(&source, b"the same content")?;

        let (size, hash, staging) = stage_blob(open_regular(&source)?, &blobs)?;
        place_blob(&staging, &blobs, (size, &hash))?;
        let blob = blobs.join(hex(&hash));
        let first = std::fs::metadata(&blob)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
        let (_, again, staging) = stage_blob(open_regular(&source)?, &blobs)?;
        place_blob(&staging, &blobs, (size, &again))?;
        let second = std::fs::metadata(&blob)?;

        // A resumed download names the blob by size and modification time.
        assert_eq!((size, again), (16, hash));
        assert_eq!(first.modified()?, second.modified()?);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(first.ino(), second.ino());
        }
        assert!(!staging.exists(), "the second copy is discarded");
        assert_eq!(std::fs::read_dir(&blobs)?.count(), 1);

        // A stored blob damaged in place is replaced by sound bytes published again.
        std::fs::write(&blob, b"the some content")?;
        let (_, _, staging) = stage_blob(open_regular(&source)?, &blobs)?;
        place_blob(&staging, &blobs, (size, &hash))?;
        assert_eq!(std::fs::read(&blob)?, b"the same content");
        assert_eq!(std::fs::read_dir(&blobs)?.count(), 1);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}
