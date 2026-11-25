//! `ma2a file`: remote Files operations and verified transfers.

use std::io::{self, Read as _, Write as _};

use ma2a_core::service::{
    FileMetadata, FilesRequest, FilesResult, MAX_SMALL_FILE_LEN, OpenBody, ServiceError, WriteMode,
};
use ma2a_runtime::ipc::IpcPaths;
use ma2a_services::client::{self, ClientError};

use super::{open, remote_path, transfer};
use crate::{
    AppError,
    cli::services::{FileArgs, FileCommand},
};

pub(crate) async fn run(paths: IpcPaths, arguments: FileArgs) -> Result<(), AppError> {
    let target = (&paths, arguments.endpoint.as_str());
    match arguments.command {
        FileCommand::Stat { path } => {
            let (result, _) = files(target, FilesRequest::Stat(remote_path(&path)?), &[]).await?;
            if let FilesResult::Metadata(metadata) = result {
                print_metadata(&metadata)?;
            }
            Ok(())
        }
        FileCommand::Ls { path } => list(target, &path).await,
        FileCommand::Mkdir { path, parents } => {
            done(target, FilesRequest::Mkdir(remote_path(&path)?, parents)).await
        }
        FileCommand::Rm { path, recursive } => {
            done(target, FilesRequest::Remove(remote_path(&path)?, recursive)).await
        }
        FileCommand::Mv { from, to, replace } => {
            let request = FilesRequest::Rename(remote_path(&from)?, remote_path(&to)?, replace);
            done(target, request).await
        }
        FileCommand::Cat { path } => cat(target, &path).await,
        FileCommand::Write { path, replace } => write(target, &path, replace).await,
        FileCommand::Get {
            path,
            local,
            resume,
            force,
        } => transfer::get(target, (&path, &local), (resume, force)).await,
        FileCommand::Put {
            local,
            path,
            replace,
        } => transfer::put(target, (&local, &path), replace).await,
    }
}

type Target<'a> = (&'a IpcPaths, &'a str);

async fn files(
    (paths, endpoint): Target<'_>,
    request: FilesRequest,
    content: &[u8],
) -> Result<(FilesResult, Vec<u8>), AppError> {
    let (mut reader, mut writer) = open(paths, endpoint, OpenBody::Files(request.clone())).await?;
    Ok(client::files::run((&mut reader, &mut writer), &request, content).await?)
}

async fn done(target: Target<'_>, request: FilesRequest) -> Result<(), AppError> {
    files(target, request, &[]).await.map(|_| ())
}

/// Whether a page lies strictly after the cursor sent, and so does the cursor it names next;
/// a target answering otherwise would keep a listing going forever.
fn moves_past(
    after: Option<&str>,
    (mut listed, next): (impl Iterator<Item = impl AsRef<str>>, Option<&str>),
) -> bool {
    after.is_none_or(|after| {
        listed.all(|name| name.as_ref() > after) && next.is_none_or(|next| next > after)
    })
}

async fn list(target: Target<'_>, path: &str) -> Result<(), AppError> {
    let directory = remote_path(path)?;
    let mut cursor: Option<String> = None;
    loop {
        let request = FilesRequest::List(directory.clone(), cursor.clone());
        let (result, _) = files(target, request, &[]).await?;
        let FilesResult::Listing(entries, next) = result else {
            return Err(io::Error::other("target answered a listing with another result").into());
        };
        let names = entries.iter().map(|entry| entry.name.as_str());
        if !moves_past(cursor.as_deref(), (names, next.as_deref())) {
            return Err(ClientError::Stream(ServiceError::InvalidInput).into());
        }
        let mut stdout = io::stdout().lock();
        for entry in entries {
            let metadata = entry.metadata;
            writeln!(
                stdout,
                "{:<9} {:>14} {}",
                metadata.kind.name(),
                metadata.size,
                entry.name
            )?;
        }
        match next {
            Some(next) => cursor = Some(next),
            None => return Ok(()),
        }
    }
}

/// Prints a file through bounded reads; `get` verifies content end to end instead.
async fn cat(target: Target<'_>, path: &str) -> Result<(), AppError> {
    let path = remote_path(path)?;
    let mut offset = 0_u64;
    loop {
        let request = FilesRequest::Read(path.clone(), offset, MAX_SMALL_FILE_LEN);
        let (result, bytes) = files(target, request, &[]).await?;
        let FilesResult::Read(length, eof) = result else {
            return Err(io::Error::other("target answered a read with another result").into());
        };
        io::stdout().lock().write_all(&bytes)?;
        offset = offset.saturating_add(u64::from(length));
        if eof || length == 0 {
            return Ok(io::stdout().lock().flush()?);
        }
    }
}

async fn write(target: Target<'_>, path: &str, replace: bool) -> Result<(), AppError> {
    let limit = u64::from(MAX_SMALL_FILE_LEN) + 1;
    let mut content = Vec::new();
    io::stdin().lock().take(limit).read_to_end(&mut content)?;
    let length = u32::try_from(content.len())
        .ok()
        .filter(|length| *length <= MAX_SMALL_FILE_LEN)
        .ok_or(AppError::Usage(
            "file write takes at most 64 KiB; use `ma2a file put`",
        ))?;
    let mode = if replace {
        WriteMode::Replace
    } else {
        WriteMode::CreateNew
    };
    let request = FilesRequest::Write(remote_path(path)?, mode, length);
    files(target, request, &content).await.map(|_| ())
}

fn print_metadata(metadata: &FileMetadata) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "Kind: {}", metadata.kind.name())?;
    writeln!(stdout, "Size: {}", metadata.size)?;
    if let Some(modified) = metadata.modified_ms {
        writeln!(stdout, "Modified (Unix ms): {modified}")?;
    }
    writeln!(stdout, "Read-only: {}", metadata.read_only)
}

#[cfg(test)]
mod tests {
    use super::moves_past;

    /// Whether a page listing `listed` and naming `next` moves past `after`.
    fn moves(after: Option<&str>, listed: &[&str], next: Option<&str>) -> bool {
        moves_past(after, (listed.iter().copied(), next))
    }

    #[test]
    fn a_listing_page_must_move_past_the_cursor_sent() {
        // The first page has nothing to move past.
        assert!(moves(None, &["b"], Some("b")));
        assert!(moves(Some("b"), &["c", "d"], Some("d")));
        assert!(moves(Some("b"), &["c"], None));
        // The cursor may name an entry the host left out.
        assert!(moves(Some("b"), &[], Some("c")));
        // The same cursor again, one behind it, or an entry already listed.
        assert!(!moves(Some("b"), &["c"], Some("b")));
        assert!(!moves(Some("b"), &[], Some("a")));
        assert!(!moves(Some("b"), &["b"], None));
    }
}
