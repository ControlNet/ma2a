//! Artifacts of this Runtime's own tasks, read through bounded local API chunks.

use std::path::Path;

use base64::Engine as _;
use ma2a_core::service::ContentIdentity;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;

use ma2a_services::client::resume::Partial;

use super::agents::{Runtime, payload};
use crate::AppError;

/// One decoded `agent_task_artifact` chunk.
pub(crate) struct Chunk {
    /// Size of the whole artifact.
    pub(crate) size: u64,
    /// Lowercase hex BLAKE3 of the whole artifact.
    pub(crate) blake3: String,
    pub(crate) content: Vec<u8>,
    /// Whether this chunk reaches the end of the artifact.
    pub(crate) eof: bool,
}

/// Reads the chunk of one local task's artifact that starts at `offset`.
pub(crate) async fn read(
    runtime: &Runtime<'_>,
    (task, name): (&str, &str),
    offset: u64,
) -> Result<Chunk, AppError> {
    let fields = [
        ("task_id".to_owned(), json!(task)),
        ("name".to_owned(), json!(name)),
        ("offset".to_owned(), json!(offset)),
    ]
    .into_iter()
    .collect();
    let chunk = payload(runtime, "agent_task_artifact_read", fields).await?;
    decode(&chunk).ok_or_else(|| AppError::Protocol("malformed artifact chunk".to_owned()))
}

fn decode(chunk: &Value) -> Option<Chunk> {
    let content = chunk.get("content").and_then(Value::as_str)?;
    Some(Chunk {
        size: chunk.pointer("/artifact/size").and_then(Value::as_u64)?,
        blake3: chunk
            .pointer("/artifact/blake3")
            .and_then(Value::as_str)?
            .to_owned(),
        content: base64::engine::general_purpose::STANDARD
            .decode(content)
            .ok()?,
        eof: chunk.get("eof").and_then(Value::as_bool)?,
    })
}

/// Downloads one artifact into a new file, verified against its hash, and prints where.
///
/// The bytes go to a partial file in the private download directory beside `output`, held
/// with its directories from the start; only a complete, verified copy is published as
/// `output`, which is never overwritten.
pub(crate) async fn save(
    runtime: &Runtime<'_>,
    (task, name): (&str, &str),
    output: &Path,
) -> Result<(), AppError> {
    refuse_existing(output)?;
    // Fetch the first chunk before opening `.ma2a-downloads`, so an unknown task or artifact
    // never truncates a paused `file get` targeting the same path.
    let first = read(runtime, (task, name), 0).await?;
    let (size, hash) = (first.size, first.blake3.clone());
    let source = format!("local agent task {task} artifact {name}");
    let mut partial = Partial::open(output, &source, false).await?;
    let copied = async {
        partial.begin(ContentIdentity::new(size, 0)).await?;
        let mut chunk = first;
        let mut offset = 0_u64;
        loop {
            if (chunk.size, &chunk.blake3) != (size, &hash)
                || (chunk.content.is_empty() && !chunk.eof)
            {
                return Err(AppError::Protocol(
                    "the artifact changed while read".to_owned(),
                ));
            }
            let chunk_len = u64::try_from(chunk.content.len()).unwrap_or(u64::MAX);
            offset = offset
                .checked_add(chunk_len)
                .ok_or_else(|| AppError::Protocol("the artifact failed verification".to_owned()))?;
            if offset > size {
                return Err(AppError::Protocol(
                    "the artifact failed verification".to_owned(),
                ));
            }
            let (file, hasher) = partial.sink();
            hasher.update(&chunk.content);
            file.write_all(&chunk.content).await?;
            if chunk.eof {
                break;
            }
            chunk = read(runtime, (task, name), offset).await?;
        }
        if offset != size || partial.sink().1.finalize().to_hex().as_str() != hash {
            return Err(AppError::Protocol(
                "the artifact failed verification".to_owned(),
            ));
        }
        Ok((size, hash))
    }
    .await;
    let (size, hash) = match copied {
        Ok(copied) => copied,
        Err(error) => {
            let _discarded = partial.discard().await;
            return Err(error);
        }
    };
    partial.finish_or_discard(false).await?;
    let saved = json!({"output": output.display().to_string(), "size": size, "blake3": hash});
    super::agents::print(&saved)
}

/// Refuses an `output` that already exists before anything is downloaded; publishing refuses
/// one that appears meanwhile.
pub(crate) fn refuse_existing(output: &Path) -> Result<(), AppError> {
    match std::fs::symlink_metadata(output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("`{}` already exists", output.display()),
        )
        .into()),
    }
}
