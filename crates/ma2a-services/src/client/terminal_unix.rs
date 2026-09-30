//! Unix terminal control through `termios` and `SIGWINCH`.

use std::{io, os::fd::AsFd as _};

use rustix::termios::{self, OptionalActions, Termios};
use tokio::signal::unix::{Signal, SignalKind, signal};

/// Puts stdin's terminal in raw mode until dropped, then restores it exactly.
#[derive(Debug)]
pub struct RawMode {
    original: Termios,
}

impl RawMode {
    /// Enables raw mode, or returns `None` when stdin is not a terminal.
    ///
    /// # Errors
    /// Returns the `termios` failure of a terminal that refused the change.
    pub fn enable() -> io::Result<Option<Self>> {
        let stdin = io::stdin();
        let fd = stdin.as_fd();
        if !termios::isatty(fd) {
            return Ok(None);
        }
        let original = termios::tcgetattr(fd)?;
        let mut raw = original.clone();
        raw.make_raw();
        termios::tcsetattr(fd, OptionalActions::Now, &raw)?;
        Ok(Some(Self { original }))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _restored =
            termios::tcsetattr(io::stdin().as_fd(), OptionalActions::Now, &self.original);
    }
}

/// Returns the columns and rows of the terminal on stdout.
pub(super) fn size() -> Option<(u16, u16)> {
    let size = termios::tcgetwinsize(io::stdout().as_fd()).ok()?;
    Some((size.ws_col, size.ws_row))
}

/// Terminal resize notifications.
pub(super) struct ResizeEvents(Signal);

impl ResizeEvents {
    pub(super) fn new() -> io::Result<Self> {
        signal(SignalKind::window_change()).map(Self)
    }

    /// Waits for the next resize; `false` once notifications stop.
    pub(super) async fn next(&mut self) -> bool {
        self.0.recv().await.is_some()
    }
}
