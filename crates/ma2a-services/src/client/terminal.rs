//! The local terminal of an interactive Shell: raw mode, its size, and resize events.

use ma2a_core::service::TerminalSize;
use tokio::sync::mpsc;

#[cfg(unix)]
#[path = "terminal_unix.rs"]
mod platform;

#[cfg(windows)]
#[path = "terminal_windows.rs"]
mod platform;

pub use platform::RawMode;

/// How often a platform without resize notifications re-reads the terminal size.
#[cfg(windows)]
pub(crate) const RESIZE_POLL: std::time::Duration = std::time::Duration::from_millis(250);

/// Resize events are coalesced, so a burst never queues more than a few sizes.
const RESIZE_QUEUE: usize = 4;

/// Returns the size of the terminal on stdout, or `None` when stdout is not a terminal.
pub fn size() -> Option<TerminalSize> {
    let (cols, rows) = platform::size()?;
    TerminalSize::new(cols, rows).ok()
}

/// Streams each new terminal size until the receiver is dropped.
///
/// # Errors
/// Returns a local error when resize notifications cannot be installed.
pub fn resizes() -> std::io::Result<mpsc::Receiver<TerminalSize>> {
    let (sender, receiver) = mpsc::channel(RESIZE_QUEUE);
    let mut changed = platform::ResizeEvents::new()?;
    tokio::spawn(async move {
        let mut last = size();
        while changed.next().await {
            let current = size();
            if current.is_some() && current != last {
                last = current;
                if let Some(current) = current
                    && sender.send(current).await.is_err()
                {
                    return;
                }
            }
        }
    });
    Ok(receiver)
}
