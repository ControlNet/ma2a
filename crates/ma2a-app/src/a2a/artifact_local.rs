//! Local artifact reading and verification for the A2A adapter.

use std::io;

use axum::{
    body::{Body, Bytes},
    http::StatusCode,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{Chunks, matches_reference};
use crate::{
    AppError,
    a2a::backend::{Backend, Refusal},
    commands::agent_artifact,
};

/// Maps a failure reading the first chunk of a local artifact to its HTTP status.
fn read_status(error: AppError) -> StatusCode {
    match error {
        AppError::Rejected(name, _) if name == "not_found" => StatusCode::NOT_FOUND,
        AppError::Rejected(name, _) => {
            if Refusal(name).is_transient() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
        AppError::Protocol(_) => StatusCode::INTERNAL_SERVER_ERROR,
        // Any transport or daemon-reachability error on the local API may clear on retry.
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Verifies one local chunk against `reference` (`(size, blake3)`) and advances `(offset, hasher)`
/// past its bytes; returns `None` on any mismatch.
fn verify_chunk(
    chunk: &agent_artifact::Chunk,
    reference: (u64, &str),
    (offset, hasher): (u64, &mut blake3::Hasher),
) -> Option<u64> {
    if (chunk.size, chunk.blake3.as_str()) != reference || (chunk.content.is_empty() && !chunk.eof)
    {
        return None;
    }
    let next = u64::try_from(chunk.content.len())
        .ok()
        .and_then(|len| offset.checked_add(len))?;
    if next > reference.0 {
        return None;
    }
    hasher.update(&chunk.content);
    let verified = if chunk.eof {
        matches_reference(reference, (next, hasher.finalize().as_bytes()))
    } else {
        next < reference.0
    };
    verified.then_some(next)
}

/// Reads and checks chunk 0 of a local artifact before sending HTTP `200`, verifying the whole
/// artifact immediately when chunk 0 reaches `eof`.
pub(super) async fn open(
    backend: Backend,
    (task, name): (String, String),
    (size, blake3): (u64, String),
) -> Result<Body, StatusCode> {
    let runtime = (backend.state_dir.as_path(), backend.paths.clone());
    let chunk = agent_artifact::read(&runtime, (&task, &name), 0)
        .await
        .map_err(read_status)?;
    let mut hasher = blake3::Hasher::new();
    let Some(offset) = verify_chunk(&chunk, (size, blake3.as_str()), (0, &mut hasher)) else {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    };
    if chunk.eof {
        return Ok(Body::from(chunk.content));
    }
    let (chunks, received) = mpsc::channel(4);
    if chunks.send(Ok(Bytes::from(chunk.content))).await.is_ok() {
        tokio::spawn(async move {
            let tail = Tail {
                backend: &backend,
                task: &task,
                name: &name,
                size,
                blake3: &blake3,
                offset,
                hasher,
            };
            if let Err(error) = stream(tail, &chunks).await {
                let _sent = chunks.send(Err(error)).await;
            }
        });
    }
    Ok(Body::from_stream(ReceiverStream::new(received)))
}

struct Tail<'a> {
    backend: &'a Backend,
    task: &'a str,
    name: &'a str,
    size: u64,
    blake3: &'a str,
    offset: u64,
    hasher: blake3::Hasher,
}

/// Reads the remaining chunks of a local artifact from `tail.offset` into `chunks`, verifying
/// the whole before forwarding the last chunk.
async fn stream(mut tail: Tail<'_>, chunks: &Chunks) -> io::Result<()> {
    let runtime = (tail.backend.state_dir.as_path(), tail.backend.paths.clone());
    loop {
        let chunk = agent_artifact::read(&runtime, (tail.task, tail.name), tail.offset)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        let Some(next) = verify_chunk(
            &chunk,
            (tail.size, tail.blake3),
            (tail.offset, &mut tail.hasher),
        ) else {
            return Err(io::Error::other("the artifact failed verification"));
        };
        tail.offset = next;
        if chunks.send(Ok(Bytes::from(chunk.content))).await.is_err() || chunk.eof {
            return Ok(());
        }
    }
}
