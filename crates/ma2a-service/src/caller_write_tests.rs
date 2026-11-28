use std::{
    pin::Pin,
    task::{Context, Poll},
};

use ma2a_core::service::{Channel, FrameKind, ServiceError};
use ma2a_services::FrameWriter;

use super::{control, flush_call};
use crate::{
    Error,
    call::{Channels, Party},
};

/// An [`AsyncWrite`] that fails after `remaining` bytes have been written, and fails on flush.
struct FailsAfterWrite(usize);

impl tokio::io::AsyncWrite for FailsAfterWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.0 == 0 {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let take = buffer.len().min(self.0);
        self.0 -= take;
        Poll::Ready(Ok(take))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

const fn unknown(result: &Result<(), Error>) -> bool {
    matches!(result, Err(Error::Service(ServiceError::OutcomeUnknown)))
}

#[tokio::test]
async fn open_and_mid_call_write_or_flush_failures_report_outcome_unknown()
-> Result<(), Box<dyn std::error::Error>> {
    // Open keeps Cancelled only when 0 bytes left the writer; any sent byte or failed flush is OutcomeUnknown.
    let mut writer = FrameWriter::new(FailsAfterWrite(0));
    assert!(matches!(
        writer.write_open(b"open").await,
        Err(ServiceError::Cancelled)
    ));
    for remaining in [2, 5, usize::MAX] {
        let mut writer = FrameWriter::new(FailsAfterWrite(remaining));
        assert_eq!(
            writer.write_open(b"open").await,
            Err(ServiceError::OutcomeUnknown),
            "open remaining={remaining}"
        );
    }

    // Mid-call writes (send, end, control, cancel) always report OutcomeUnknown on write/flush failure.
    for remaining in [0, 3, usize::MAX] {
        let mut writer = FrameWriter::new(FailsAfterWrite(remaining));
        let mut ended = Channels::default();
        ended.take((Channel::Input, Party::Caller), false)?;
        let wrote = writer.data(Channel::Input, b"hi").await;
        assert!(
            unknown(&flush_call(&mut writer, wrote).await),
            "send remaining={remaining}"
        );

        let mut writer = FrameWriter::new(FailsAfterWrite(remaining));
        let mut ended = Channels::default();
        ended.take((Channel::Input, Party::Caller), true)?;
        let wrote = writer.end(Channel::Input).await;
        assert!(
            unknown(&flush_call(&mut writer, wrote).await),
            "end remaining={remaining}"
        );

        let mut writer = FrameWriter::new(FailsAfterWrite(remaining));
        assert!(
            unknown(&control(&mut writer, b"c").await),
            "control remaining={remaining}"
        );

        let mut writer = FrameWriter::new(FailsAfterWrite(remaining));
        let wrote = writer.frame(FrameKind::Cancel, &[]).await;
        assert!(
            unknown(&flush_call(&mut writer, wrote).await),
            "cancel remaining={remaining}"
        );
    }

    // Validation errors stay InvalidInput on flush_call.
    let mut writer = FrameWriter::new(FailsAfterWrite(0));
    let wrote = writer.data(Channel::Input, b"").await;
    assert!(matches!(
        flush_call(&mut writer, wrote).await,
        Err(Error::Service(ServiceError::InvalidInput))
    ));
    Ok(())
}
