//! Artifacts of this Runtime's own tasks, read through bounded local API chunks.

use std::path::Path;

use base64::Engine as _;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt as _;

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

/// The sibling a download of `output` is written to first: its whole name plus
/// `.ma2a-partial`, so two outputs never share one.
pub(crate) fn partial_of(output: &Path) -> std::path::PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(".ma2a-partial");
    output.with_file_name(name)
}

/// Downloads one artifact into a new file, verified against its hash, and prints where.
///
/// The bytes go to a sibling `.ma2a-partial` file first; only a complete, verified copy is
/// linked to `output`, which is never overwritten.
pub(crate) async fn save(
    runtime: &Runtime<'_>,
    (task, name): (&str, &str),
    output: &Path,
) -> Result<(), AppError> {
    let partial = partial_of(output);
    let mut file = tokio::fs::File::create_new(&partial).await?;
    let copied = async {
        let (mut offset, mut hasher, mut expected) = (0_u64, blake3::Hasher::new(), None);
        loop {
            let chunk = read(runtime, (task, name), offset).await?;
            let (size, hash) = expected.get_or_insert_with(|| (chunk.size, chunk.blake3.clone()));
            if (chunk.size, &chunk.blake3) != (*size, &*hash)
                || (chunk.content.is_empty() && !chunk.eof)
            {
                return Err(AppError::Protocol(
                    "the artifact changed while read".to_owned(),
                ));
            }
            hasher.update(&chunk.content);
            file.write_all(&chunk.content).await?;
            offset += u64::try_from(chunk.content.len()).unwrap_or(u64::MAX);
            if chunk.eof {
                break;
            }
        }
        let (size, hash) = expected.unwrap_or_default();
        if offset != size || hasher.finalize().to_hex().as_str() != hash {
            return Err(AppError::Protocol(
                "the artifact failed verification".to_owned(),
            ));
        }
        file.flush().await?;
        file.sync_all().await?;
        tokio::fs::hard_link(&partial, output).await?;
        Ok((size, hash))
    }
    .await;
    drop(file);
    let _removed = tokio::fs::remove_file(&partial).await;
    let (size, hash) = copied?;
    let saved = json!({"output": output.display().to_string(), "size": size, "blake3": hash});
    super::agents::print(&saved)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::partial_of;

    #[test]
    fn a_partial_download_keeps_the_whole_output_name() {
        assert_eq!(
            partial_of(Path::new("/tmp/out/report.txt")),
            Path::new("/tmp/out/report.txt.ma2a-partial")
        );
        assert_ne!(
            partial_of(Path::new("report.txt")),
            partial_of(Path::new("report.bin"))
        );
    }
}
