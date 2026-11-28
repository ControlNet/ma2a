use std::{
    pin::Pin,
    task::{Context, Poll},
};

use ma2a_core::service::ServiceError;
use ma2a_services::{FrameWriter, client::ClientError};

use super::send_open;

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

#[tokio::test]
async fn open_write_or_flush_failures_after_sent_bytes_report_outcome_unknown() {
    let mut writer = FrameWriter::new(FailsAfterWrite(0));
    assert!(matches!(
        send_open(&mut writer, b"open").await,
        Err(ClientError::Stream(ServiceError::Cancelled))
    ));
    for remaining in [2, 5, usize::MAX] {
        let mut writer = FrameWriter::new(FailsAfterWrite(remaining));
        assert!(
            matches!(
                send_open(&mut writer, b"open").await,
                Err(ClientError::Stream(ServiceError::OutcomeUnknown))
            ),
            "remaining={remaining}"
        );
    }
}
