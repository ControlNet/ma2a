//! One inbound stream: coarse admission, bounded `open`, fine authorization, host dispatch.

use std::time::Duration;

use ma2a_core::ServiceKind;
use ma2a_core::service::{
    DescribePage, FrameKind, OpenBody, OpenHeader, ServiceError, ServiceRequest, TransferRequest,
    admission,
};
use ma2a_net::{RecvStream, SendStream, ServiceCall};
use ma2a_services::{FrameReader, FrameWriter, OperationEnd, OperationReport, reply};
use tokio_util::sync::CancellationToken;

use super::{
    HostContext,
    journal::StoreJournal,
    operations::{Direction, OperationKey, operation_name},
};

/// Longest wait for the `open` frame after admission.
const OPEN_DEADLINE: Duration = Duration::from_secs(15);
/// Longest duration of one content-free or small Files operation.
const FILES_DEADLINE: Duration = Duration::from_secs(15);

type Reader<'a> = FrameReader<&'a mut RecvStream>;
type Writer<'a> = FrameWriter<&'a mut SendStream>;

pub(super) async fn serve(call: ServiceCall, context: HostContext) {
    let peer = call.remote_endpoint_id();
    let kind = call.kind();
    let Some(access) = admission(kind) else {
        call.reject(ServiceError::Unavailable);
        return;
    };
    if let Err(error) = context
        .store
        .authorize_service((context.local, peer), access)
        .await
    {
        call.reject(error);
        return;
    }
    let mut stream = call.authorize();
    let (send, receive) = stream.halves();
    let mut reader = FrameReader::new(receive);
    let mut writer = FrameWriter::new(send);
    let header = match read_open(&mut reader).await {
        Ok(header) => header,
        Err(error) => {
            refuse(&mut writer, error).await;
            return;
        }
    };
    let key = OperationKey {
        request_id: header.request_id(),
        peer,
        kind,
        operation: operation_name(header.body()),
        direction: Direction::Inbound,
    };
    if header.target() != context.local || header.body().kind() != kind {
        reject(&mut writer, (&context, key), ServiceError::InvalidInput).await;
        return;
    }
    let access = header.body().access();
    let stop = context.shutdown.child_token();
    let watch = context
        .authorizations
        .watch((peer, access.clone()), stop.clone());
    let grant = match context
        .store
        .authorize_service((context.local, peer), access)
        .await
    {
        Ok(grant) => grant,
        // Past admission, an Agent or a hosted service this caller may not use answers
        // exactly like one that does not exist.
        Err(ServiceError::Unauthorized) if kind == ServiceKind::Agent => {
            reject(&mut writer, (&context, key), ServiceError::UnknownAgent).await;
            return;
        }
        Err(ServiceError::Unauthorized) if kind == ServiceKind::Service => {
            reject(&mut writer, (&context, key), ServiceError::UnknownService).await;
            return;
        }
        Err(error) => {
            reject(&mut writer, (&context, key), error).await;
            return;
        }
    };
    watch.bind(grant.root.clone());
    let guard = context.operations.start(key);
    let mut report = if stop.is_cancelled() {
        stopped(&mut writer, ServiceError::Unavailable)
    } else {
        dispatch(
            (header, grant.root, peer),
            (&context, &stop),
            (&mut reader, &mut writer),
        )
        .await
    };
    drop(watch);
    if writer.is_torn() || (stop.is_cancelled() && !writer.terminal_sent()) {
        // A deadline or stop abandoned a stream without a terminal frame: the peer must see a
        // reset, never a clean end. The report keeps its cause.
        let _reset = writer.get_mut().reset(0_u8.into());
    }
    if stop.is_cancelled() && !context.shutdown.is_cancelled() {
        // Authorization was withdrawn while the operation ran.
        report.end = OperationEnd::Failed(ServiceError::Unauthorized);
    }
    guard.finish(report);
}

/// Ends a stream whose host was stopped before it could answer.
fn stopped(writer: &mut Writer<'_>, error: ServiceError) -> OperationReport {
    let _reset = writer.get_mut().reset(0_u8.into());
    OperationReport::new(OperationEnd::Failed(error))
}

/// Sends `reject`; one cut short by its deadline leaves the stream reset, never cleanly ended.
async fn refuse(writer: &mut Writer<'_>, error: ServiceError) {
    reply::reject(writer, error).await;
    if writer.is_torn() {
        let _reset = writer.get_mut().reset(0_u8.into());
    }
}

async fn read_open(reader: &mut Reader<'_>) -> Result<OpenHeader, ServiceError> {
    let frame = tokio::time::timeout(OPEN_DEADLINE, reader.expect())
        .await
        .map_err(|_| ServiceError::TimedOut)??;
    if frame.kind() != FrameKind::Open {
        return Err(ServiceError::InvalidInput);
    }
    OpenHeader::decode(frame.payload()).map_err(ServiceError::from_protocol)
}

async fn reject(
    writer: &mut Writer<'_>,
    (context, key): (&HostContext, OperationKey),
    error: ServiceError,
) {
    refuse(writer, error).await;
    context
        .operations
        .record(key, OperationReport::new(OperationEnd::Rejected(error)));
}

/// Serves the decoded request; `stop` is cancelled on shutdown or withdrawn authorization.
async fn dispatch(
    (header, root, peer): (
        OpenHeader,
        Option<std::path::PathBuf>,
        ma2a_core::EndpointId,
    ),
    (context, stop): (&HostContext, &CancellationToken),
    (reader, writer): (&mut Reader<'_>, &mut Writer<'_>),
) -> OperationReport {
    let request_id = header.request_id();
    // Exec, Shell, and Proxy observe `stop` themselves and still send their outcome, unless a
    // frame was abandoned part-way, in which case the stream is reset afterwards.
    let shutdown = stop;
    match (header.into_body(), root) {
        (OpenBody::Files(request), Some(root)) => {
            let served = tokio::select! {
                biased;
                () = stop.cancelled() => Err(ServiceError::Unavailable),
                served = tokio::time::timeout(
                    FILES_DEADLINE,
                    ma2a_services::serve_files(root, request, (reader, writer)),
                ) => served.map_err(|_| ServiceError::TimedOut),
            };
            served.unwrap_or_else(|error| stopped(writer, error))
        }
        (
            OpenBody::Transfer(request @ (TransferRequest::Get(..) | TransferRequest::Put(..))),
            Some(root),
        ) => {
            let journal = StoreJournal {
                store: context.store.clone(),
                request_id,
            };
            let served = tokio::select! {
                biased;
                () = stop.cancelled() => None,
                served = ma2a_services::serve_transfer(
                    (root, request_id),
                    (request, &journal),
                    (reader, writer),
                ) => Some(served),
            };
            served.unwrap_or_else(|| stopped(writer, ServiceError::Unavailable))
        }
        (OpenBody::Exec(request), None) => {
            ma2a_services::serve_exec(request, shutdown, (reader, writer)).await
        }
        (OpenBody::Shell(request), None) => {
            ma2a_services::serve_shell(request, shutdown, (reader, writer)).await
        }
        (OpenBody::Proxy(request), None) => {
            ma2a_services::serve_proxy(request, shutdown, (reader, writer)).await
        }
        (OpenBody::Process(request), None) => {
            let operation = ((peer, request_id), request, stop);
            crate::processes::serve(&context.processes, operation, (reader, writer)).await
        }
        (OpenBody::Agent(request), None) => {
            let operation = ((peer, request_id), request, stop);
            crate::agents::serve(&context.agents, operation, (reader, writer)).await
        }
        (OpenBody::Service(ServiceRequest::Describe(after)), None) => {
            let page = context.hosted.describe(peer, after).await;
            describe(writer, page).await
        }
        (OpenBody::Service(ServiceRequest::Call(call)), None) => {
            let operation = ((peer, request_id), call, stop);
            crate::hosting::serve(&context.hosted, operation, (reader, writer)).await
        }
        _ => reply::reject(writer, ServiceError::Internal).await,
    }
}

/// Answers `describe`: its one outcome, or its refusal before any effect, each written within
/// the grace a host gives a caller that stops reading.
async fn describe(
    writer: &mut Writer<'_>,
    page: Result<DescribePage, ServiceError>,
) -> OperationReport {
    match page
        .as_ref()
        .map(|page| DescribePage::encode_outcome(Ok(page)))
    {
        Ok(Ok(encoded)) => reply::outcome(writer, (Ok(encoded), None), (0, 0)).await,
        Ok(Err(_)) => reply::reject(writer, ServiceError::Internal).await,
        Err(error) => reply::reject(writer, *error).await,
    }
}
