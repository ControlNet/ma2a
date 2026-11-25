//! One hosted call between its hand-over to the service and its end, as the caller sees it.

use ma2a_core::{
    EndpointId, RequestId,
    service::{
        FrameKind, HostedCall, OperationName, ServiceError, ServiceName, encode_call_outcome,
    },
};
use ma2a_services::{FrameReader, FrameWriter, OperationEnd, OperationReport};
use tokio::{
    io::{AsyncReadExt as _, DuplexStream, ReadHalf, WriteHalf},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::hosted::{named, register};
use super::services::{Harness, call, text};
use super::*;
use crate::{
    hosting::{Accepted, HostedServices, Notice},
    ipc::BUSINESS_CONNECTION_LIMIT,
};

type Ends = (
    FrameReader<ReadHalf<DuplexStream>>,
    FrameWriter<WriteHalf<DuplexStream>>,
);

/// Both framed ends of one in-memory stream with `capacity` bytes of room.
fn pipe(capacity: usize) -> (Ends, Ends) {
    let (one, two) = tokio::io::duplex(capacity);
    let ((one_read, one_write), (two_read, two_write)) =
        (tokio::io::split(one), tokio::io::split(two));
    (
        (FrameReader::new(one_read), FrameWriter::new(one_write)),
        (FrameReader::new(two_read), FrameWriter::new(two_write)),
    )
}

/// A call being served to a caller, announced to its ready service and not yet accepted.
struct Announced {
    harness: Harness,
    _link: crate::hosting::LinkHandle,
    hosted: HostedServices,
    name: ServiceName,
    id: [u8; 16],
    stop: CancellationToken,
    caller: Ends,
    serving: JoinHandle<TestResult<OperationReport>>,
}

async fn announce(label: &str, room: usize) -> TestResult<Announced> {
    let harness = Harness::start(label).await?;
    call(&harness.client, register(&"81".repeat(16))?).await?;
    call(&harness.client, named("service_start", &"82".repeat(16))).await?;
    let hosted = harness.hosted()?;
    let name = ServiceName::new("example")?;
    let (mut link, _manifest) = hosted.attach(&name).await?;
    hosted.report((&name, link.id), true);
    let request = HostedCall::new((name.clone(), 1), OperationName::new("echo")?, Vec::new())?;
    let ((caller, served), stop) = (pipe(room), CancellationToken::new());
    let serving = tokio::spawn({
        let (hosted, stop) = (hosted.clone(), stop.clone());
        async move {
            let (mut reader, mut writer) = served;
            let peer = EndpointId::from(iroh::SecretKey::generate().public());
            let operation = ((peer, RequestId::random()?), request, &stop);
            Ok(crate::hosting::serve(&hosted, operation, (&mut reader, &mut writer)).await)
        }
    });
    let Some(Notice::Call(id)) = link.next().await else {
        return Err("no call notice".into());
    };
    Ok(Announced {
        harness,
        _link: link,
        hosted,
        name,
        id,
        stop,
        caller,
        serving,
    })
}

impl Announced {
    /// Hands the call to the service over a new connection, whose service end is returned.
    fn deliver(&self, stopping: bool) -> TestResult<DuplexStream> {
        let (_details, deliver) = self
            .hosted
            .accept(&self.name, self.id)
            .map_err(|error| format!("{error:?}"))?;
        if stopping {
            self.stop.cancel();
        }
        let (service, runtime_side) = tokio::io::duplex(1024);
        let _delivered = deliver.send(Accepted {
            stream: Box::new(runtime_side),
            answer: b"details".to_vec(),
        });
        Ok(service)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_stopped_while_its_service_accepts_it_is_refused_untouched() -> TestResult {
    // Given
    let announced = announce("hosted-accept-stop", 1024).await?;

    // When: the service takes the call just as the operation stops.
    let mut service = announced.deliver(true)?;
    announced.serving.await??;

    // Then: the caller is refused, so nothing is left unknown, and the service heard nothing.
    let (mut reader, _writer) = announced.caller;
    let answered = reader.expect().await?;
    assert_eq!(answered.kind(), FrameKind::Reject, "{answered:?}");
    assert_eq!(answered.rejection()?, ServiceError::Unavailable);
    let mut heard = Vec::new();
    service.read_to_end(&mut heard).await?;
    assert!(heard.is_empty(), "{heard:?}");
    announced.harness.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_successful_outcome_the_caller_never_takes_is_not_audited_as_completed() -> TestResult {
    // Given: a caller that reads nothing, through a stream with no room.
    let announced = announce("hosted-untaken-outcome", 1).await?;
    let service = announced.deliver(false)?;
    let (mut service_read, service_write) = tokio::io::split(service);
    let mut answer = [0_u8; 7];
    service_read.read_exact(&mut answer).await?;
    let mut to_runtime = FrameWriter::new(service_write);

    // When: the service ends the call successfully.
    let outcome = encode_call_outcome(Ok(b"done"))?;
    to_runtime.frame(FrameKind::Outcome, &outcome).await?;
    let report = announced.serving.await??;

    // Then: the audit says the outcome was never delivered.
    assert_eq!(report.end, OperationEnd::Failed(ServiceError::TimedOut));
    announced.harness.stop().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_accept_is_admitted_while_every_business_slot_is_taken() -> TestResult {
    // Given: a call waiting for its service, and every business slot held by other local work.
    let announced = announce("hosted-accept-reserve", 1024).await?;
    let client = &announced.harness.client;
    let held = Arc::clone(&announced.harness.slots)
        .acquire_many_owned(u32::try_from(BUSINESS_CONNECTION_LIMIT)?)
        .await?;

    // When: the service accepts the call.
    let accept = serde_json::json!({
        "version": 1, "operation": "service_accept", "service": "example",
        "call_id": api::encode_hex(&announced.id),
    });
    let accept = api::decode_command(&serde_json::to_vec(&accept)?)?;
    let opened = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.open_hosted(&accept),
    )
    .await??;

    // Then: it is handed the call at once, while other local requests are still turned away.
    let answer: serde_json::Value = serde_json::from_slice(&opened.0)?;
    assert_eq!(
        text(&answer, "/result/type"),
        Some("service_call"),
        "{answer}"
    );
    assert!(opened.1.is_some(), "the accepted call has no stream");
    assert!(
        matches!(client.probe().await, Err(IpcError::Api(error)) if error.code() == ProtocolError::UNAVAILABLE),
        "a handshake must not take a slot kept for service_accept"
    );
    drop((opened, held));
    announced.harness.stop().await
}
