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

use ma2a_core::{
    RequestId,
    service::{
        AgentTaskId, ArtifactRef, ContentIdentity, RemotePath, RootName, ServiceError,
        TransferRequest, TransferResource,
    },
};
use ma2a_services::{FrameReader, FrameWriter, OperationReport};
use ma2a_store::AgentTaskChange;
use tokio::io::{AsyncRead, AsyncWrite};

use super::{AgentManager, convert::hex, host::refuse, private_dir, tasks::Viewer};
use crate::stream_host::journal::StoreJournal;

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

/// Copies `source` into the blob store and returns its size and hash.
fn store_blob(source: &Path, blobs: &Path) -> io::Result<(u64, [u8; 32])> {
    private_dir(blobs)?;
    let mut input = std::fs::File::open(source)?;
    if !input.metadata()?.is_file() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
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
    let published = copied.and_then(|(size, hash)| {
        std::fs::rename(&staging, blobs.join(hex(&hash)))?;
        Ok((size, hash))
    });
    if published.is_err() {
        let _removed = std::fs::remove_file(&staging);
    }
    published
}

impl AgentManager {
    /// Publishes one workspace file as an artifact of a live task.
    pub(super) async fn publish(
        &self,
        task: AgentTaskId,
        (name, source, media_type): (&str, PathBuf, &str),
    ) -> Result<ArtifactRef, ServiceError> {
        let blobs = self.blobs();
        let stored = tokio::task::spawn_blocking(move || store_blob(&source, &blobs))
            .await
            .map_err(|_| ServiceError::Internal)?;
        let (size, hash) = stored.map_err(|error| match error.kind() {
            io::ErrorKind::FileTooLarge => ServiceError::TooLarge,
            io::ErrorKind::NotFound => ServiceError::NotFound,
            _ => ServiceError::InvalidInput,
        })?;
        let artifact = ArtifactRef::new((name, media_type), (size, hash))
            .map_err(|_| ServiceError::InvalidInput)?;
        let (recorded, at_ms) = (artifact.clone(), self.now_ms());
        match self
            .commit(move |repository| repository.add_agent_artifact(task, (&recorded, at_ms)))
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
        ((viewer, (task, name), (offset, identity)), request_id): (ArtifactRequest<'_>, RequestId),
        (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    ) -> OperationReport
    where
        R: AsyncRead + Unpin + Send,
        W: AsyncWrite + Unpin + Send,
    {
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
        let request = TransferRequest::Get(TransferResource::File(path), offset, identity);
        let journal = StoreJournal {
            store: self.0.store.clone(),
            request_id,
        };
        ma2a_services::serve_transfer(
            (self.blobs(), request_id),
            (request, &journal),
            (reader, writer),
        )
        .await
    }

    /// Removes every old blob no retained task references.
    pub(super) async fn collect_blobs(&self) {
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
