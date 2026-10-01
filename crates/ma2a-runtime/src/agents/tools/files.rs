//! `files.*`, `transfer.*`, and `artifact.publish`.

use std::path::{Path, PathBuf};

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
use ma2a_services::{FrameReader, client};
use tokio::io::{AsyncRead, AsyncWriteExt as _};

use super::{
    Call, Outcome, failed, ok, status,
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

/// Returns a hidden sibling to download into, published by rename only once verified.
fn staging(destination: &Path) -> Result<PathBuf, ToolError> {
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(|_| status(ServiceError::Internal))?;
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(destination.with_file_name(format!(".{name}.{}.partial", hex(&suffix))))
}

impl Call<'_> {
    async fn files(&self, endpoint: &str, request: FilesRequest) -> Result<FilesResult, ToolError> {
        let body = OpenBody::Files(request.clone());
        let (mut reader, mut writer) = self.open_read(endpoint, body).await?;
        let (result, _) = client::files::run((&mut reader, &mut writer), &request, &[])
            .await
            .map_err(|error| failed(&error))?;
        Ok(result)
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
        let target = destination(&self.workspace, &args.to)?;
        let request = TransferRequest::Get(resource, 0, None);
        let (reader, _writer) = self
            .open_read(&args.endpoint, OpenBody::Transfer(request))
            .await?;
        ok(&transferred(download(reader, &target).await?))
    }

    pub(super) async fn transfer_put(&self, args: TransferPut) -> Outcome {
        let source = existing(&self.workspace, &args.from)?;
        let resource = TransferResource::File(remote(&args.root, &args.path)?);
        let mut file = crate::agents::artifacts::open_regular(&source)
            .map(tokio::fs::File::from_std)
            .map_err(|error| local(&error))?;
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
            .publish(self.task, (&args.name, source, media_type))
            .await
            .map_err(status)?;
        ok(&artifact(&published))
    }
}

/// Downloads one accepted Transfer stream into `target`, verified before it is published.
pub(super) async fn download<R: AsyncRead + Unpin>(
    mut reader: FrameReader<R>,
    target: &Path,
) -> Result<TransferResult, ToolError> {
    client::transfer::download_accepted(&mut reader)
        .await
        .map_err(|error| failed(&error))?;
    let partial = staging(target)?;
    let downloaded = async {
        let mut file = tokio::fs::File::create_new(&partial)
            .await
            .map_err(|error| local(&error))?;
        let mut hasher = blake3::Hasher::new();
        let result = client::transfer::download_content(&mut reader, (&mut file, &mut hasher))
            .await
            .map_err(|error| failed(&error))?;
        file.flush().await.map_err(|error| local(&error))?;
        file.sync_all().await.map_err(|error| local(&error))?;
        tokio::fs::rename(&partial, target)
            .await
            .map_err(|error| local(&error))?;
        Ok(result)
    }
    .await;
    if downloaded.is_err() {
        let _removed = tokio::fs::remove_file(&partial).await;
    }
    downloaded
}
