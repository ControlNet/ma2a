//! `ma2a file get` and `ma2a file put`: verified, resumable downloads and committed uploads.

use std::{io, path::Path};

use ma2a_core::service::{OpenBody, ServiceError, TransferRequest, TransferResource, WriteMode};
use ma2a_services::client::{self, ClientError, resume::Partial};

use super::{open, remote_path};
use crate::AppError;

type Target<'a> = (&'a ma2a_runtime::ipc::IpcPaths, &'a str);

/// Downloads `path` to `local`, restarting once if a resumed source changed.
pub(super) async fn get(
    target: Target<'_>,
    (path, local): (&str, &Path),
    (resume, force): (bool, bool),
) -> Result<(), AppError> {
    let resource = TransferResource::File(remote_path(path)?);
    if !force && tokio::fs::try_exists(local).await? {
        return Err(AppError::Invalid(
            "the local file exists; pass --force to replace it".to_owned(),
        ));
    }
    crate::daemon_control::require(target.0).await?;
    // The sidecar binds a partial file to the exact target Endpoint and remote path.
    let source = format!("{} {path}", target.1);
    let mut partial = Partial::open(local, &source, resume).await?;
    match attempt(target, &resource, &mut partial).await? {
        Err(ClientError::Rejected(ServiceError::PreconditionFailed)) if partial.offset() > 0 => {
            // The source changed since the partial download began: start over once.
            partial.discard().await?;
            partial = Partial::open(local, &source, false).await?;
            if let Err(error) = attempt(target, &resource, &mut partial).await? {
                return Err(abandon(partial, error).await);
            }
        }
        Err(error) => return Err(abandon(partial, error).await),
        Ok(()) => {}
    }
    partial.finish(force).await.map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            AppError::Invalid(
                "the local file appeared meanwhile; pass --force to replace it".to_owned(),
            )
        } else {
            error.into()
        }
    })
}

/// Runs one download attempt; the outer error means the stream never opened.
async fn attempt(
    (paths, endpoint): Target<'_>,
    resource: &TransferResource,
    partial: &mut Partial,
) -> Result<Result<(), ClientError>, AppError> {
    let request = TransferRequest::Get(resource.clone(), partial.offset(), partial.identity());
    let (mut reader, _writer) = open(paths, endpoint, OpenBody::Transfer(request)).await?;
    Ok(async {
        let identity = client::transfer::download_accepted(&mut reader).await?;
        partial.begin(identity).await?;
        client::transfer::download_content(&mut reader, partial.sink()).await?;
        Ok(())
    }
    .await)
}

/// Keeps a partial download worth resuming and removes one that is not.
///
/// Content that changed or failed verification cannot be continued, and an attempt that
/// failed before its first byte left nothing of value.
async fn abandon(partial: Partial, error: ClientError) -> AppError {
    let worthless = partial.identity().is_none()
        || matches!(
            error,
            ClientError::Failed(ServiceError::ContentChanged | ServiceError::IntegrityMismatch)
        );
    if worthless && let Err(removed) = partial.discard().await {
        return removed.into();
    }
    error.into()
}

/// Uploads `local` to `path`, published only after the target verified size and hash.
pub(super) async fn put(
    (paths, endpoint): Target<'_>,
    (local, path): (&Path, &str),
    replace: bool,
) -> Result<(), AppError> {
    let mut file = tokio::fs::File::open(local).await?;
    let size = file.metadata().await?.len();
    let mode = if replace {
        WriteMode::Replace
    } else {
        WriteMode::CreateNew
    };
    let request = TransferRequest::Put(TransferResource::File(remote_path(path)?), size, mode);
    let (mut reader, mut writer) = open(paths, endpoint, OpenBody::Transfer(request)).await?;
    client::transfer::upload((&mut reader, &mut writer), (&mut file, size)).await?;
    Ok(())
}
