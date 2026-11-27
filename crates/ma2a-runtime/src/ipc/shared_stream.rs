use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::platform::{PlatformStream, peer_disconnected};

/// Shared handle to one accepted local socket stream so peer disconnect can be observed
/// concurrently with framed reading and writing.
#[derive(Clone, Debug)]
pub(crate) struct SharedStream(Arc<PlatformStream>);

impl SharedStream {
    pub(crate) fn new(stream: PlatformStream) -> Self {
        Self(Arc::new(stream))
    }

    /// Resolves once the connected peer disconnects or shuts down its sending half.
    pub(crate) async fn peer_disconnected(&self) {
        peer_disconnected(&self.0).await;
    }
}

impl AsyncRead for SharedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for SharedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self.0).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut &*self.0).poll_shutdown(cx)
    }
}
