//! One inbound call of a hosted service: admission, hand-over to the service, and the relay.
//!
//! The service learns a call only when it accepts it, so until then a call can be withdrawn
//! with no possible effect. After that the existing stream-plane relay moves frames verbatim
//! and enforces their direction rules; a stop or a withdrawn authorization reaches the
//! service as a `cancel`, and a lost service connection ends as `outcome_unknown`.

mod relay;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use ma2a_core::{
    EndpointId, RequestId,
    service::{FrameKind, HostedCall, ServiceError, encode_call_outcome},
};
use ma2a_services::{FrameReader, FrameWriter, OperationEnd, OperationReport};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt as _},
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;

use super::{Accepted, CallDetails, HostedServices, Notice, Pending, fault};
use crate::stream_plane::relay::{Terminal, bounded};

/// Longest wait for a service to accept a call it was told about.
const ACCEPT_DEADLINE: Duration = Duration::from_secs(10);

/// Releases one call's slot, and withdraws it if it is still pending, however it ends.
struct Slot {
    manager: HostedServices,
    running: Arc<AtomicUsize>,
    call: [u8; 16],
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut state = self.manager.state();
        state.pending.remove(&self.call);
        self.running.fetch_sub(1, Ordering::Relaxed);
        drop(state);
    }
}

/// An admitted call: its slot, its hand-over, and the stop of its service.
type Admitted = (Slot, oneshot::Receiver<Accepted>, CancellationToken);

/// Serves one authorized call for `peer`; `stop` is cancelled on shutdown or withdrawal.
pub(crate) async fn serve<R, W>(
    manager: &HostedServices,
    ((peer, request_id), call, stop): ((EndpointId, RequestId), HostedCall, &CancellationToken),
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
) -> OperationReport
where
    R: AsyncRead + Unpin + Send,
    W: AsyncWrite + Unpin + Send,
{
    let (slot, mut delivered, service_stop) = match manager.admit(call, (peer, request_id)).await {
        Ok(admitted) => admitted,
        Err(error) => return refuse(writer, error).await,
    };
    let waited = tokio::select! {
        biased;
        accepted = &mut delivered => Some(accepted),
        () = tokio::time::sleep(ACCEPT_DEADLINE) => None,
        () = stop.cancelled() => None,
        () = service_stop.cancelled() => None,
    };
    let accepted = match waited {
        Some(accepted) => accepted.ok(),
        None if manager.withdraw(slot.call) => None,
        // The service is accepting it right now: the hand-over completes or is withdrawn.
        None => delivered.await.ok(),
    };
    let Some(Accepted { mut stream, answer }) = accepted else {
        return refuse(writer, ServiceError::Unavailable).await;
    };
    let written = async {
        stream.write_all(&answer).await?;
        stream.flush().await
    }
    .await;
    fault("after_call_delivered");
    if written.is_err() {
        return lost(writer, true).await;
    }
    let relayed = relay::relay_call((reader, &mut *writer), stream, (stop, &service_stop)).await;
    drop(slot);
    let report = match relayed.terminal {
        Terminal::Rejected(error) => OperationReport::new(OperationEnd::Rejected(error)),
        Terminal::Outcome { failure, .. } => {
            OperationReport::new(failure.map_or(OperationEnd::Completed, OperationEnd::Failed))
        }
        Terminal::Lost => lost(writer, relayed.client_present).await,
    };
    let _closed = bounded(writer.shutdown()).await;
    report.with_bytes(relayed.to_target, relayed.from_target)
}

impl HostedServices {
    /// Admits one call of a ready service within its `max_calls`, and tells the service.
    async fn admit(
        &self,
        call: HostedCall,
        (caller, request_id): (EndpointId, RequestId),
    ) -> Result<Admitted, ServiceError> {
        let service = call.service().clone();
        let record = self
            .registration(&service)
            .await?
            .ok_or(ServiceError::UnknownService)?;
        let version = record
            .manifest
            .version(call.version())
            .ok_or(ServiceError::VersionMismatch)?;
        if version.operation(call.operation().as_str()).is_none() {
            return Err(ServiceError::InvalidInput);
        }
        if !record.enabled {
            return Err(ServiceError::Unavailable);
        }
        let mut id = [0_u8; 16];
        getrandom::fill(&mut id).map_err(|_| ServiceError::Internal)?;
        let mut state = self.state();
        let live = state
            .services
            .get_mut(&service)
            .ok_or(ServiceError::Unavailable)?;
        let link = live
            .link
            .as_ref()
            .filter(|link| link.ready)
            .ok_or(ServiceError::Unavailable)?;
        if live.running.load(Ordering::Relaxed) >= usize::from(record.manifest.max_calls()) {
            return Err(ServiceError::ConcurrencyExceeded);
        }
        link.notices
            .try_send(Notice::Call(id))
            .map_err(|_| ServiceError::Unavailable)?;
        live.running.fetch_add(1, Ordering::Relaxed);
        let running = Arc::clone(&live.running);
        let count = live.calls.entry(call.version()).or_default();
        *count = count.saturating_add(1);
        let service_stop = live.stop.clone();
        let (deliver, delivered) = oneshot::channel();
        let details = CallDetails {
            caller,
            request_id,
            version: call.version(),
            operation: call.operation().clone(),
            body: call.body().to_vec(),
        };
        state.pending.insert(
            id,
            Pending {
                service,
                details,
                deliver,
            },
        );
        drop(state);
        let slot = Slot {
            manager: self.clone(),
            running,
            call: id,
        };
        Ok((slot, delivered, service_stop))
    }
}

/// Refuses a call before its service learned anything.
async fn refuse<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    error: ServiceError,
) -> OperationReport {
    let _sent = bounded(writer.reject(error)).await;
    let _closed = bounded(writer.shutdown()).await;
    OperationReport::new(OperationEnd::Rejected(error))
}

/// Ends a call whose service may have acted without reporting how it ended.
async fn lost<W: AsyncWrite + Unpin + Send>(
    writer: &mut FrameWriter<W>,
    caller_present: bool,
) -> OperationReport {
    if caller_present && let Ok(outcome) = encode_call_outcome(Err(ServiceError::OutcomeUnknown)) {
        let _sent = bounded(async {
            writer.frame(FrameKind::Outcome, &outcome).await?;
            writer.flush().await
        })
        .await;
    }
    OperationReport::new(OperationEnd::Failed(ServiceError::OutcomeUnknown))
}
