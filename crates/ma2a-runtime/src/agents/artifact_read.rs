//! Bounded reads of one artifact for the local operator, one local API response at a time.

use std::io::{self, Read as _, Seek as _, SeekFrom};

use ma2a_core::service::{AgentTaskId, ArtifactRef, ServiceError};

use super::{AgentManager, convert::hex, tasks::Viewer};

/// Most artifact bytes one read returns, so the encoded chunk fits one local API response.
const MAX_ARTIFACT_READ: usize = 32_768;

impl AgentManager {
    /// Reads up to [`MAX_ARTIFACT_READ`] bytes of one artifact from `offset`.
    ///
    /// Blobs are named by their BLAKE3, so successive reads of one artifact always observe
    /// the same content; a reader verifies the whole against the returned hash.
    pub(crate) async fn read_artifact(
        &self,
        viewer: Viewer<'_>,
        ((task, name), offset): ((AgentTaskId, &str), u64),
    ) -> Result<(ArtifactRef, Vec<u8>), ServiceError> {
        let details = self.details(viewer, task).await?;
        let artifact = details
            .artifacts
            .into_iter()
            .find(|found| found.name() == name)
            .ok_or(ServiceError::UnknownArtifact)?;
        let remaining = artifact
            .size()
            .checked_sub(offset)
            .ok_or(ServiceError::InvalidInput)?;
        let length = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(MAX_ARTIFACT_READ);
        let path = self.blobs().join(hex(artifact.blake3()));
        let read = tokio::task::spawn_blocking(move || -> io::Result<Vec<u8>> {
            let mut file = std::fs::File::open(path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes)?;
            Ok(bytes)
        })
        .await
        .map_err(|_| ServiceError::Internal)?;
        // A referenced blob is never collected, so a missing one is a broken store.
        let bytes = read.map_err(|_| ServiceError::Internal)?;
        Ok((artifact, bytes))
    }
}
