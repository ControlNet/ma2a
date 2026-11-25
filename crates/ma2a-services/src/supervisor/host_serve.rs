//! Authenticated daemon connections to one supervisor, one request per connection.

use std::sync::{Arc, atomic::Ordering};

use interprocess::local_socket::tokio::{Listener, RecvHalf, SendHalf, Stream, prelude::*};
use ma2a_core::service::{FrameKind, ServiceError};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use super::{
    address::CONNECT_DEADLINE,
    host::{Lifecycle, Shared},
    proof::{prove_supervisor, read_challenge},
    protocol::{ACKNOWLEDGED, SupervisorRequest},
    status::SupervisorStatus,
};
use crate::{FrameReader, FrameWriter};

pub(super) type Reader = FrameReader<RecvHalf>;
pub(super) type Writer = FrameWriter<SendHalf>;

/// How long to wait after a failed accept, such as one at the descriptor limit, which would
/// otherwise fail again at once.
const ACCEPT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);

/// Accepts connections until `stop`.
pub(super) async fn serve(listener: Listener, shared: Arc<Shared>, stop: CancellationToken) {
    loop {
        let stream = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => match accepted {
                Ok(stream) => stream,
                Err(_) => {
                    tokio::select! {
                        () = stop.cancelled() => return,
                        () = tokio::time::sleep(ACCEPT_BACKOFF) => continue,
                    }
                }
            },
        };
        let shared = Arc::clone(&shared);
        let stop = stop.clone();
        shared.connections.fetch_add(1, Ordering::AcqRel);
        tokio::spawn(async move {
            tokio::select! {
                () = stop.cancelled() => {}
                () = connection(stream, &shared) => {}
            }
            shared.connections.fetch_sub(1, Ordering::AcqRel);
        });
    }
}

async fn connection(mut stream: Stream, shared: &Shared) {
    // Windows names a pipe client only once something was read from the pipe, so the daemon's
    // challenge is read before the caller's account is checked, and answered only after.
    let deadline = tokio::time::Instant::now() + CONNECT_DEADLINE;
    let challenge =
        tokio::time::timeout_at(deadline, read_challenge(&mut FrameReader::new(&mut stream))).await;
    let Ok(Ok(theirs)) = challenge else {
        return;
    };
    if super::address::same_user(&stream).is_err() {
        return;
    }
    let (receive, send) = stream.split();
    let (mut reader, mut writer) = (FrameReader::new(receive), FrameWriter::new(send));
    let opened = tokio::time::timeout_at(deadline, async {
        prove_supervisor(theirs, (&mut reader, &mut writer), &shared.credentials).await?;
        let frame = reader.expect().await?;
        if frame.kind() != FrameKind::Open {
            return Err(ServiceError::InvalidInput);
        }
        SupervisorRequest::from_bytes(frame.payload())
    })
    .await;
    let Ok(Ok(request)) = opened else {
        return;
    };
    let _served = serve_request(request, (&mut reader, &mut writer), shared).await;
    let _flushed = writer.shutdown().await;
}

async fn ask<T>(
    shared: &Shared,
    request: impl FnOnce(oneshot::Sender<T>) -> Lifecycle,
) -> Result<T, ServiceError> {
    let (reply, answer) = oneshot::channel();
    shared
        .requests
        .send(request(reply))
        .await
        .map_err(|_| ServiceError::Unavailable)?;
    answer.await.map_err(|_| ServiceError::Unavailable)
}

/// Answers from the status alone once the lifecycle task is gone.
fn settled(shared: &Shared) -> Result<SupervisorStatus, ServiceError> {
    let status = *shared.status.borrow();
    if status.is_final() {
        Ok(status)
    } else {
        Err(ServiceError::Unavailable)
    }
}

async fn serve_request(
    request: SupervisorRequest,
    (reader, writer): (&mut Reader, &mut Writer),
    shared: &Shared,
) -> Result<(), ServiceError> {
    match request {
        SupervisorRequest::Watch => watch(writer, shared).await,
        SupervisorRequest::Go => {
            let status = match ask(shared, Lifecycle::Go).await {
                Ok(status) => status,
                Err(_) => settled(shared)?,
            };
            writer.frame(FrameKind::Outcome, &status.to_bytes()).await
        }
        SupervisorRequest::Abort => match ask(shared, Lifecycle::Abort).await {
            Ok(Ok(status)) => writer.frame(FrameKind::Outcome, &status.to_bytes()).await,
            Ok(Err(error)) => writer.reject(error).await,
            Err(_) => {
                writer
                    .frame(FrameKind::Outcome, &settled(shared)?.to_bytes())
                    .await
            }
        },
        SupervisorRequest::Signal(signal) => {
            match ask(shared, |reply| Lifecycle::Signal(signal, reply)).await {
                Ok(Ok(())) => writer.frame(FrameKind::Outcome, ACKNOWLEDGED).await,
                Ok(Err(error)) => writer.reject(error).await,
                Err(_) => writer.reject(ServiceError::NotRunning).await,
            }
        }
        SupervisorRequest::Terminate(grace_ms) => {
            let status = match ask(shared, |reply| Lifecycle::Terminate(grace_ms, reply)).await {
                Ok(status) => status,
                Err(_) => settled(shared)?,
            };
            writer.frame(FrameKind::Outcome, &status.to_bytes()).await
        }
        SupervisorRequest::Attach(from, stdin) => {
            super::host_attach::attach((reader, writer), shared, (from, stdin)).await
        }
        SupervisorRequest::Stdin(close) => {
            super::host_stdin::stdin((reader, writer), shared, close).await
        }
    }
}

/// Streams every status change, ending with the final status as `outcome`.
async fn watch(writer: &mut Writer, shared: &Shared) -> Result<(), ServiceError> {
    let mut changes = shared.status.subscribe();
    writer.frame(FrameKind::Accept, ACKNOWLEDGED).await?;
    loop {
        let status = *changes.borrow_and_update();
        if status.is_final() {
            return writer.frame(FrameKind::Outcome, &status.to_bytes()).await;
        }
        writer.frame(FrameKind::Control, &status.to_bytes()).await?;
        writer.flush().await?;
        if changes.changed().await.is_err() {
            return Err(ServiceError::Unavailable);
        }
    }
}
