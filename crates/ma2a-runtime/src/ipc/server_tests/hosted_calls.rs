//! One hosted call between its hand-over to the service and its end, as the caller sees it.

use ma2a_core::{
    EndpointId, RequestId,
    service::{FrameKind, HostedCall, OperationName, ServiceError, ServiceName},
};
use ma2a_services::{FrameReader, FrameWriter, OperationReport};
use tokio::{
    io::{AsyncReadExt as _, DuplexStream, ReadHalf, WriteHalf},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::hosted::{named, register};
use super::services::{Harness, call};
use super::*;
use crate::hosting::{Accepted, HostedServices, Notice};

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
