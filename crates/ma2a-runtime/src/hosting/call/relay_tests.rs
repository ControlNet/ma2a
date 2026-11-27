use std::time::Duration;

use ma2a_core::service::{Channel, FrameKind};
use ma2a_services::{FrameReader, FrameWriter};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio_util::sync::CancellationToken;

use super::{INPUT_STALL, STUCK_GRACE, relay_call};
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

#[tokio::test(start_paused = true)]
async fn a_cancel_queued_behind_input_the_service_never_reads_still_ends_the_call() {
    let (caller, host) = tokio::io::duplex(1 << 20);
    let (read, write) = tokio::io::split(host);
    let (mut reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    // The service end is held open and never read.
    let (_service, hosted) = tokio::io::duplex(1 << 14);
    let (_caller_read, caller_write) = tokio::io::split(caller);
    let mut input = FrameWriter::new(caller_write);
    for _ in 0..32 {
        input
            .data(Channel::Input, &[7; 16_384])
            .await
            .expect("queue input");
    }
    input
        .frame(FrameKind::Cancel, &[])
        .await
        .expect("queue cancel");
    input.flush().await.expect("flush input");
    let (stop, service_stop) = (CancellationToken::new(), CancellationToken::new());
    let started = tokio::time::Instant::now();
    let calling = relay_call(
        (&mut reader, &mut writer),
        Box::new(hosted),
        (&stop, &service_stop),
    );
    let relayed = tokio::time::timeout(BOUND, calling)
        .await
        .expect("a cancel behind unread input never ended the call");
    assert_eq!(relayed.terminal, Terminal::Lost);
    assert!(started.elapsed() <= INPUT_STALL + STUCK_GRACE);
}

#[tokio::test(start_paused = true)]
async fn a_stopped_call_ends_when_its_caller_stopped_reading() {
    let ((mut reader, mut writer), _caller) = caller_stream();
    let (service, hosted) = tokio::io::duplex(1 << 14);
    // The service floods output that the caller never reads.
    let flood = tokio::spawn(async move {
        let mut output = FrameWriter::new(service);
        let accept = ma2a_core::service::encode_call_accept(b"").expect("accept");
        output
            .frame(FrameKind::Accept, &accept)
            .await
            .expect("accept frame");
        while output.data(Channel::Output, &[7; 16_384]).await.is_ok()
            && output.flush().await.is_ok()
        {}
    });
    let (stop, service_stop) = (CancellationToken::new(), CancellationToken::new());
    let stopping = async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        service_stop.cancel();
    };
    let calling = async {
        let relayed = relay_call(
            (&mut reader, &mut writer),
            Box::new(hosted),
            (&stop, &service_stop),
        )
        .await;
        assert_eq!(relayed.terminal, Terminal::Lost);
        // The relay was dropped while writing to the caller: nothing may follow.
        assert!(!relayed.client_present);
        super::super::lost(&mut writer, relayed.client_present).await
    };
    let (ended, ()) = tokio::join!(tokio::time::timeout(BOUND, calling), stopping);
    flood.abort();
    assert!(
        ended.is_ok(),
        "ending the call hung on a caller that stopped reading"
    );
}

#[tokio::test]
async fn a_continuous_output_flood_never_starves_stop_or_stuck_grace() {
    use tokio::io::AsyncWriteExt as _;

    let (caller, host) = tokio::io::duplex(1 << 16);
    let (host_read, _host_write) = tokio::io::split(host);
    let mut reader = FrameReader::new(host_read);
    let mut writer = FrameWriter::new(tokio::io::sink());
    let (_caller_read, caller_write) = tokio::io::split(caller);
    let input_flood = tokio::spawn(async move {
        let mut input = FrameWriter::new(caller_write);
        while input.data(Channel::Input, &[7; 16_384]).await.is_ok() && input.flush().await.is_ok()
        {
        }
    });

    let (service, hosted) = tokio::io::duplex(1 << 14);
    let mut batch = FrameWriter::new(Vec::new());
    for _ in 0..256 {
        batch
            .data(Channel::Output, &[b'x'; 64])
            .await
            .expect("output data");
    }
    let batch = batch.into_inner();
    let output_flood = tokio::spawn(async move {
        let mut output = FrameWriter::new(service);
        let accept = ma2a_core::service::encode_call_accept(b"").expect("accept");
        output
            .frame(FrameKind::Accept, &accept)
            .await
            .expect("accept");
        output.flush().await.expect("flush accept");
        while output.get_mut().write_all(&batch).await.is_ok() {}
    });

    let (stop, service_stop) = (CancellationToken::new(), CancellationToken::new());
    let stopping = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        service_stop.cancel();
    };
    let started = tokio::time::Instant::now();
    let calling = relay_call(
        (&mut reader, &mut writer),
        Box::new(hosted),
        (&stop, &service_stop),
    );
    let (relayed, ()) = tokio::join!(
        tokio::time::timeout(STUCK_GRACE + Duration::from_secs(3), calling),
        stopping
    );
    input_flood.abort();
    output_flood.abort();
    let relayed = relayed.expect("stopped call starved under continuous output");
    assert_eq!(relayed.terminal, Terminal::Lost);
    assert!(started.elapsed() <= STUCK_GRACE + Duration::from_secs(2));
}
