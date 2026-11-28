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
use ma2a_core::service::{AgentRequest, ContentIdentity, OpenBody, ServiceError};
use ma2a_runtime::ipc::LocalStream;
use ma2a_services::{FrameReader, FrameWriter, client};
use serde_json::Value;
use tokio::{
    io::{AsyncWrite, DuplexStream, ReadHalf, WriteHalf},
    sync::{mpsc, oneshot},
};
use tokio_stream::Stream;
use tokio_util::io::ReaderStream;

use super::{
    backend::{Backend, Refusal, Route},
    http::status,
    remote,
};
use crate::commands::remote::open_as;

#[path = "artifact_local.rs"]
mod local;

/// Pipe capacity between a remote download and the response body.
const PIPE: usize = 65_536;

type Chunks = mpsc::Sender<Result<Bytes, io::Error>>;

/// Serves one artifact of a task of the route's Agent.
pub(super) async fn serve(
    backend: Backend,
    (route, task, name): (Route, String, String),
) -> Response {
    let details = match backend.details(&route, &task).await {
        Ok(details) => details,
        // A Runtime or target that cannot answer now may answer later; any other refusal,
        // one hiding a task this caller may not see included, names no artifact.
        Err(refusal) if refusal.is_transient() => {
            return status(StatusCode::SERVICE_UNAVAILABLE);
        }
        Err(_) => return status(StatusCode::NOT_FOUND),
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
    let blake3 = artifact
        .get("blake3")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let is_local = matches!(route, Route::Local(_));
    let body = match route {
        Route::Local(_) => match local::open(backend, (task, name), (size, blake3)).await {
            Ok(body) => body,
            Err(code) => return status(code),
        },
        Route::Remote(..) => {
            let accepted = match admit(&backend, (&route, &task, &name)).await {
                Ok(accepted) => accepted,
                Err(refusal) if refusal.is_transient() => {
                    return status(StatusCode::SERVICE_UNAVAILABLE);
                }
                Err(_) => return status(StatusCode::NOT_FOUND),
            };
            let (mut writer, reader) = tokio::io::duplex(PIPE);
            let (outcome, verdict) = oneshot::channel();
            tokio::spawn(relay(outcome, async move {
                let streamed = download(accepted, (size, blake3.as_str()), &mut writer).await;
                drop(writer);
                streamed
            }));
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

/// Runs one remote download into a response body and reports whether it verified. A client
/// that left ends it, even while the target sends nothing, so a stalled target does not keep
/// the stream open for nobody.
async fn relay(
    mut outcome: oneshot::Sender<bool>,
    download: impl Future<Output = Result<(), Refusal>>,
) {
    let streamed = tokio::select! {
        biased;
        () = outcome.closed() => return,
        streamed = download => streamed,
    };
    let _sent = outcome.send(streamed.is_ok());
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

type Accepted = (
    FrameReader<ReadHalf<LocalStream>>,
    FrameWriter<WriteHalf<LocalStream>>,
    ContentIdentity,
);

/// Opens a remote artifact download and waits up to [`remote::CALL_DEADLINE`] for the target
/// to admit it.
async fn admit(
    backend: &Backend,
    (route, task, name): (&Route, &str, &str),
) -> Result<Accepted, Refusal> {
    let (endpoint, agent) = remote::parts(route)?;
    let request = AgentRequest::Artifact(
        agent.clone(),
        remote::task_id(task)?,
        name.to_owned(),
        0,
        None,
    );
    remote::bounded(remote::CALL_DEADLINE, async {
        let (mut reader, writer) =
            open_as(&backend.paths, (endpoint, None), OpenBody::Agent(request)).await?;
        let identity = client::transfer::download_accepted(&mut reader)
            .await
            .map_err(|error| remote::refusal(&error))?;
        Ok((reader, writer, identity))
    })
    .await
}

/// Streams one admitted artifact's verified content into `sink`. Content that is not what
/// `reference` names, the size and BLAKE3 hex of the task's artifact reference, is refused:
/// a wrong size before any byte, a wrong hash at the end.
async fn download<S: AsyncWrite + Unpin>(
    (mut reader, _writer, identity): Accepted,
    reference: (u64, &str),
    sink: &mut S,
) -> Result<(), Refusal> {
    let mismatch = || Refusal::named(ServiceError::IntegrityMismatch.name());
    if identity.size() != reference.0 {
        return Err(mismatch());
    }
    let mut hasher = blake3::Hasher::new();
    let result =
        client::transfer::download_content(&mut reader, (sink, &mut hasher), (identity, 0))
            .await
            .map_err(|error| remote::refusal(&error))?;
    if !matches_reference(reference, (result.size, &result.hash)) {
        return Err(mismatch());
    }
    Ok(())
}

/// Whether content of `size` and `hash` is the content `reference` names; a reference whose
/// hash is not BLAKE3 hex names no content.
fn matches_reference((size, blake3): (u64, &str), (got, hash): (u64, &[u8; 32])) -> bool {
    size == got && blake3::Hash::from_hex(blake3).is_ok_and(|expected| expected == *hash)
}

#[cfg(all(test, unix))]
#[path = "artifact_local_error_tests.rs"]
mod local_error_tests;
#[cfg(all(test, unix))]
#[path = "artifact_local_tests.rs"]
mod local_tests;
#[cfg(test)]
#[path = "artifact_tests.rs"]
mod tests;
