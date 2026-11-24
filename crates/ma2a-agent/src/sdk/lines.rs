//! The stdio line transport: bounded reads of Runtime lines and the queued writer.

use std::{io, sync::atomic::Ordering};

use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _},
    sync::{mpsc, oneshot},
};

use super::Line;
use crate::protocol::MAX_RUNTIME_LINE_LEN;

/// Reads one line without its newline, or `None` at end of input; a line over the bound
/// is consumed and returned empty, so it is skipped.
pub(super) async fn read_line<R: AsyncBufRead + Unpin>(
    input: &mut R,
) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut overflow = false;
    loop {
        let buffer = input.fill_buf().await?;
        if buffer.is_empty() {
            // A last line without its newline still counts as one line.
            return Ok((!line.is_empty() || overflow).then_some(line));
        }
        let (chunk, done) = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or((buffer.len(), false), |end| (end, true));
        if !overflow {
            line.extend_from_slice(buffer.get(..chunk).unwrap_or_default());
            if line.len() > MAX_RUNTIME_LINE_LEN {
                overflow = true;
                line = Vec::new();
            }
        }
        input.consume(if done { chunk + 1 } else { chunk });
        if done {
            return Ok(Some(line));
        }
    }
}

/// Writes lines as they are queued; once `stop` fires, writes what is queued and returns.
pub(super) async fn write_lines<W: AsyncWrite + Unpin>(
    mut output: W,
    mut lines: mpsc::UnboundedReceiver<Line>,
    mut stop: oneshot::Receiver<()>,
) -> io::Result<()> {
    loop {
        let line = tokio::select! {
            biased;
            line = lines.recv() => line,
            _stopped = &mut stop => {
                while let Ok((line, _)) = lines.try_recv() {
                    output.write_all(&line).await?;
                }
                return output.flush().await;
            }
        };
        let Some((line, queued)) = line else {
            return Ok(());
        };
        output.write_all(&line).await?;
        output.flush().await?;
        if let Some(queued) = queued {
            queued.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
