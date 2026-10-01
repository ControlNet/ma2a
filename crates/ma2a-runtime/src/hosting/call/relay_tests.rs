use std::time::Duration;

use ma2a_core::service::{Channel, FrameKind};
use ma2a_services::{FrameReader, FrameWriter};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

use super::{STUCK_GRACE, relay_call};
use crate::stream_plane::relay::{CANCEL_GRACE, Terminal};

/// Far past every grace; the clock is paused, so a hang fails at once instead of hanging.
const BOUND: Duration = Duration::from_secs(60);

type Host = (
    FrameReader<ReadHalf<DuplexStream>>,
    FrameWriter<WriteHalf<DuplexStream>>,
);

/// The hosting Runtime's end of one caller stream, and the caller's end.
fn caller_stream() -> (Host, DuplexStream) {
    let (caller, host) = tokio::io::duplex(1 << 16);
    let (read, write) = tokio::io::split(host);
    ((FrameReader::new(read), FrameWriter::new(write)), caller)
}

#[tokio::test(start_paused = true)]
async fn a_stopped_call_ends_even_when_its_service_stopped_reading() {
    let ((mut reader, mut writer), caller) = caller_stream();
    // The service end is held open and never read.
    let (_service, hosted) = tokio::io::duplex(1 << 14);
    let (_caller_read, caller_write) = tokio::io::split(caller);
    let flood = tokio::spawn(async move {
        let mut input = FrameWriter::new(caller_write);
        while input.data(Channel::Input, &[7; 16_384]).await.is_ok() && input.flush().await.is_ok()
        {
        }
    });
    let (stop, service_stop) = (CancellationToken::new(), CancellationToken::new());
    let stopping = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        service_stop.cancel();
    };
    let started = tokio::time::Instant::now();
    let calling = relay_call(
        (&mut reader, &mut writer),
        Box::new(hosted),
        (&stop, &service_stop),
    );
    let (relayed, ()) = tokio::join!(tokio::time::timeout(BOUND, calling), stopping);
    let relayed = relayed.expect("the stopped call never ended");
    assert_eq!(relayed.terminal, Terminal::Lost);
    assert!(relayed.client_present);
    assert!(started.elapsed() <= Duration::from_secs(1) + STUCK_GRACE);
    flood.abort();
}

#[tokio::test(start_paused = true)]
async fn a_service_ignoring_cancel_is_lost_after_the_cancel_grace() {
    let ((mut reader, mut writer), caller) = caller_stream();
    let (service, hosted) = tokio::io::duplex(1 << 14);
    // The service reads everything and never answers.
    let ignoring = tokio::spawn(async move {
        let mut frames = FrameReader::new(service);
        while let Ok(Some(_)) = frames.next().await {}
    });
    let (_caller_read, caller_write) = tokio::io::split(caller);
    let mut input = FrameWriter::new(caller_write);
    let cancelled = async {
        input.frame(FrameKind::Cancel, &[]).await?;
        input.flush().await
    };
    assert!(cancelled.await.is_ok());
    let (stop, service_stop) = (CancellationToken::new(), CancellationToken::new());
    let started = tokio::time::Instant::now();
    let calling = relay_call(
        (&mut reader, &mut writer),
        Box::new(hosted),
        (&stop, &service_stop),
    );
    let relayed = tokio::time::timeout(BOUND, calling)
        .await
        .expect("the cancelled call never ended");
    assert_eq!(relayed.terminal, Terminal::Lost);
    assert!(relayed.client_present);
    assert!(started.elapsed() <= CANCEL_GRACE + Duration::from_millis(100));
    ignoring.abort();
    drop(input);
}
