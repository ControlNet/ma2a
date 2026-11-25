//! `files.*`, `transfer.*`, and `artifact.publish`.

use std::{io, path::Path};

use ma2a_agent::{
    protocol::ToolError,
    tools::{
        ArtifactPublish, FileInfo, FilesTarget, ListedEntry, Listing, TransferGet, TransferPut,
        Transferred,
    },
};
use ma2a_core::service::{
    FileMetadata, FilesRequest, FilesResult, OpenBody, RemotePath, ServiceError, TransferRequest,
    TransferResource, TransferResult, WriteMode,
};
use ma2a_services::{client, client::resume::Partial};

use super::{
    Call, Outcome, bounded, failed, ok, status,
    workspace::{destination, existing, local},
};
use crate::agents::convert::{artifact, hex};

/// Media type of an artifact published without one.
const DEFAULT_MEDIA_TYPE: &str = "application/octet-stream";

fn remote(root: &str, path: &str) -> Result<RemotePath, ToolError> {
    RemotePath::parse(&format!("{root}:{path}"))
        .map_err(|_| ToolError::new("invalid_tool", "invalid root or remote path"))
}

fn info(metadata: &FileMetadata) -> FileInfo {
    FileInfo {
        kind: metadata.kind.name().to_owned(),
        size: metadata.size,
        modified_ms: metadata.modified_ms,
        read_only: metadata.read_only,
    }
}

fn transferred(result: TransferResult) -> Transferred {
    Transferred {
        size: result.size,
        blake3: hex(&result.hash),
    }
}

impl Call<'_> {
    async fn files(&self, endpoint: &str, request: FilesRequest) -> Result<FilesResult, ToolError> {
        bounded(async {
            let body = OpenBody::Files(request.clone());
            let (mut reader, mut writer) = self.open_read(endpoint, body).await?;
            let (result, _) = client::files::run((&mut reader, &mut writer), &request, &[])
                .await
                .map_err(|error| failed(&error))?;
            Ok(result)
        })
        .await
    }

    pub(super) async fn files_stat(&self, args: FilesTarget) -> Outcome {
        let request = FilesRequest::Stat(remote(&args.root, &args.path)?);
        match self.files(&args.endpoint, request).await? {
            FilesResult::Metadata(metadata) => ok(&info(&metadata)),
            _ => Err(status(ServiceError::InvalidInput)),
        }
    }

    pub(super) async fn files_list(&self, args: FilesTarget) -> Outcome {
        let request = FilesRequest::List(remote(&args.root, &args.path)?, args.after);
        let FilesResult::Listing(entries, next) = self.files(&args.endpoint, request).await? else {
            return Err(status(ServiceError::InvalidInput));
        };
        let entries = entries
            .into_iter()
            .map(|entry| ListedEntry {
                kind: entry.metadata.kind.name().to_owned(),
                size: entry.metadata.size,
                name: entry.name,
            })
            .collect();
        ok(&Listing { entries, next })
    }

    pub(super) async fn transfer_get(&self, args: TransferGet) -> Outcome {
        let resource = TransferResource::File(remote(&args.root, &args.path)?);
        let _target = destination(&self.workspace, &args.to)?;
        let request = TransferRequest::Get(resource, 0, None);
        let body = OpenBody::Transfer(request);
        ok(&transferred(
            self.download((&args.endpoint, body), &args.to).await?,
        ))
    }

    pub(super) async fn transfer_put(&self, args: TransferPut) -> Outcome {
        let source = existing(&self.workspace, &args.from)?;
        let resource = TransferResource::File(remote(&args.root, &args.path)?);
        let mut file = tokio::fs::File::from_std(source);
        let size = file.metadata().await.map_err(|error| local(&error))?.len();
        let mode = if args.replace {
            WriteMode::Replace
        } else {
            WriteMode::CreateNew
        };
        let request = TransferRequest::Put(resource, size, mode);
        let (mut reader, mut writer) = self
            .open(&args.endpoint, OpenBody::Transfer(request))
            .await?;
        let result = client::transfer::upload((&mut reader, &mut writer), (&mut file, size))
            .await
            .map_err(|error| failed(&error))?;
        ok(&transferred(result))
    }

    pub(super) async fn artifact_publish(&self, args: ArtifactPublish) -> Outcome {
        let source = existing(&self.workspace, &args.path)?;
        let media_type = args.media_type.as_deref().unwrap_or(DEFAULT_MEDIA_TYPE);
        let published = self
            .manager
            .publish((self.task, self.id), (&args.name, source, media_type))
            .await
            .map_err(status)?;
        ok(&artifact(&published))
    }
}

impl Call<'_> {
    /// Downloads one Transfer stream into `relative`, verified before it is published.
    ///
    /// A download is read-only, so opening its stream and waiting for its Accept share one read
    /// deadline; the content that follows has none. The content goes to a partial file in the
    /// private download directory beside the target, held with its directories from the start,
    /// so the file published is the one whose content was verified; a concurrent download to
    /// the same target is refused.
    pub(super) async fn download(
        &self,
        (endpoint, body): (&str, OpenBody),
        relative: &str,
    ) -> Result<TransferResult, ToolError> {
        let ((mut reader, _writer), identity) = bounded(async {
            let (mut reader, writer) = self.open_read(endpoint, body).await?;
            let identity = client::transfer::download_accepted(&mut reader)
                .await
                .map_err(|error| failed(&error))?;
            Ok(((reader, writer), identity))
        })
        .await?;
        // Re-validate immediately before opening the partial file so a directory component
        // swapped into a symlink during the remote handshake cannot escape the workspace.
        let workspace = &self.workspace;
        let target = destination(workspace, relative)?;
        let mut partial = partial((workspace, relative), &target).await?;
        if let Err(error) = partial.begin(identity).await {
            let _discarded = partial.discard().await;
            return Err(local(&error));
        }
        match client::transfer::download_content(&mut reader, partial.sink(), (identity, 0)).await {
            Ok(result) => {
                partial
                    .finish_or_discard(true)
                    .await
                    .map_err(|error| local(&error))?;
                Ok(result)
            }
            Err(error) => {
                let _discarded = partial.discard().await;
                Err(failed(&error))
            }
        }
    }
}

/// Opens the partial file of `target`. Once the destination's directory is held, `relative` is
/// checked again before any helper file is touched, so a directory swapped in meanwhile is left
/// as it was; that check's own error, such as an escape, is the answer.
async fn partial(
    (workspace, relative): (&Path, &str),
    target: &Path,
) -> Result<Partial, ToolError> {
    let (root, path) = (workspace.to_path_buf(), relative.to_owned());
    Partial::open_checked(target, ("agent tool download", false), move || {
        destination(&root, &path)
            .map(drop)
            .map_err(io::Error::other)
    })
    .await
    .map_err(|error| {
        error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<ToolError>())
            .cloned()
            .unwrap_or_else(|| local(&error))
    })
}

#[cfg(test)]
mod tests {
    use super::partial;

    #[tokio::test]
    async fn a_destination_that_escapes_once_held_is_an_invalid_tool() -> Result<(), String> {
        // Given: the target's directory exists, but the path checked once it is held escapes.
        let workspace = std::env::temp_dir().join(format!("ma2a-partial-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
        let target = workspace.join("file");

        // When
        let opened = partial((&workspace, "../file"), &target).await;
        let _cleanup = std::fs::remove_dir_all(&workspace);

        // Then
        let error = opened.err().ok_or("the escape was opened")?;
        assert_eq!(error.code, "invalid_tool");
        Ok(())
    }
}
