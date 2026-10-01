//! Artifact content at the URL of an A2A artifact's file part.
//!
//! The content streams as it is read and is verified against the artifact's BLAKE3. A
//! local artifact's last chunk is sent only once the whole verified; another Endpoint's
//! is sent chunked, and a failed Transfer v1 verification aborts the body before its end.
//! Either way a client never receives a complete body that is wrong.

use std::{
    future::Future as _,
    io,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse as _, Response},
};
use serde_json::Value;
use tokio::{
    io::DuplexStream,
    sync::{mpsc, oneshot},
};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tokio_util::io::ReaderStream;

use super::{
    backend::{Backend, Route},
    http::status,
    remote,
};
use crate::commands::agent_artifact;

/// Pipe capacity between a remote download and the response body.
const PIPE: usize = 65_536;

type Chunks = mpsc::Sender<Result<Bytes, io::Error>>;

/// Serves one artifact of a task of the route's Agent.
pub(super) async fn serve(
    backend: Backend,
    (route, task, name): (Route, String, String),
) -> Response {
    let Ok(details) = backend.details(&route, &task).await else {
        return status(StatusCode::NOT_FOUND);
    };
    let Some(artifact) = details
        .get("artifacts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|artifact| artifact.get("name").and_then(Value::as_str) == Some(name.as_str()))
        .cloned()
    else {
        return status(StatusCode::NOT_FOUND);
    };
    let size = artifact
        .get("size")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let media_type = artifact
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream")
        .to_owned();
    let is_local = matches!(route, Route::Local(_));
    let body = match route {
        Route::Local(_) => {
            let (chunks, received) = mpsc::channel(4);
            tokio::spawn(async move {
                if let Err(error) = local(&backend, (&task, &name), &chunks).await {
                    let _sent = chunks.send(Err(error)).await;
                }
            });
            Body::from_stream(ReceiverStream::new(received))
        }
        Route::Remote(..) => {
            let (mut writer, reader) = tokio::io::duplex(PIPE);
            let (outcome, verdict) = oneshot::channel();
            tokio::spawn(async move {
                let streamed =
                    remote::artifact(&backend, (&route, &task, &name), &mut writer).await;
                drop(writer);
                let _sent = outcome.send(streamed.is_ok());
            });
            Body::from_stream(Verified {
                content: ReaderStream::new(reader),
                verdict,
            })
        }
    };
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    if let Ok(media_type) = HeaderValue::from_str(&media_type) {
        headers.insert(header::CONTENT_TYPE, media_type);
    }
    if is_local {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    }
    response
}

/// Remote content, which fails at its end unless the download verified.
struct Verified {
    content: ReaderStream<DuplexStream>,
    verdict: oneshot::Receiver<bool>,
}

impl Stream for Verified {
    type Item = io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.content).poll_next(context) {
            Poll::Ready(None) => match Pin::new(&mut this.verdict).poll(context) {
                Poll::Ready(Ok(true)) => Poll::Ready(None),
                Poll::Ready(_) => Poll::Ready(Some(Err(io::Error::other(
                    "the artifact failed verification",
                )))),
                Poll::Pending => Poll::Pending,
            },
            other => other,
        }
    }
}

/// Reads a local artifact chunk by chunk into `chunks`, verifying the whole.
async fn local(backend: &Backend, (task, name): (&str, &str), chunks: &Chunks) -> io::Result<()> {
    let runtime = (backend.state_dir.as_path(), backend.paths.clone());
    let (mut offset, mut hasher) = (0_u64, blake3::Hasher::new());
    loop {
        let chunk = agent_artifact::read(&runtime, (task, name), offset)
            .await
            .map_err(|error| io::Error::other(error.to_string()))?;
        hasher.update(&chunk.content);
        offset += u64::try_from(chunk.content.len()).unwrap_or(u64::MAX);
        let verified = !chunk.eof
            || (offset == chunk.size && hasher.finalize().to_hex().as_str() == chunk.blake3);
        if !verified || (chunk.content.is_empty() && !chunk.eof) {
            return Err(io::Error::other("the artifact failed verification"));
        }
        if chunks.send(Ok(Bytes::from(chunk.content))).await.is_err() || chunk.eof {
            return Ok(());
        }
    }
}
