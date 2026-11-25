//! Reads of a log directory, independent of its writer.

use std::{
    fs::{self, File},
    io::{self, Read as _},
    path::Path,
};

use ma2a_core::service::LogWindow;

use super::{LogRecord, content_len, name, parse, segments};

/// Output read from a log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct LogRead {
    /// Cursor of the first returned byte.
    pub start: u64,
    /// Whether the requested cursor was older than the retained window.
    pub truncated: bool,
    /// Returned records, in order.
    pub records: Vec<LogRecord>,
    /// Cursor after the last returned byte.
    pub next: u64,
}

/// Returns the retained window of a log directory, or an empty window when there is none.
///
/// # Errors
/// Returns an I/O error when a segment cannot be read.
pub fn window(directory: &Path) -> io::Result<LogWindow> {
    for _ in 0..3 {
        let firsts = match segments(directory) {
            Ok(firsts) => firsts,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(LogWindow::default());
            }
            Err(error) => return Err(error),
        };
        let (Some(first), Some(last)) = (firsts.first(), firsts.last()) else {
            return Ok(LogWindow::default());
        };
        match fs::read(directory.join(name(*last))) {
            Ok(bytes) => return Ok(LogWindow::new(*first, last + content_len(&parse(&bytes)))),
            // Retention deleted the segment between listing and reading: list again.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound))
}

/// Reads at most `max` content bytes from `from`, starting at the window when `from` is older.
///
/// # Errors
/// Returns an I/O error when a segment cannot be read.
pub fn read(directory: &Path, from: u64, max: usize) -> io::Result<LogRead> {
    for _ in 0..3 {
        match read_once(directory, from, max) {
            // Retention deleted a segment between listing and reading: list again.
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            result => return result,
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound))
}

fn read_once(directory: &Path, from: u64, max: usize) -> io::Result<LogRead> {
    let firsts = segments(directory)?;
    let window_start = firsts.first().copied().unwrap_or(0);
    let start = from.max(window_start);
    let mut output = LogRead {
        start,
        truncated: from < window_start,
        records: Vec::new(),
        next: start,
    };
    let mut budget = max;
    for (index, first) in firsts.iter().enumerate() {
        if firsts
            .get(index + 1)
            .is_some_and(|next| *next <= output.next)
        {
            continue;
        }
        if *first > output.next {
            // A segment between is missing, so the output is not contiguous here: stop at the
            // gap, or start past it, reported as truncated, when nothing was read yet.
            if !output.records.is_empty() {
                return Ok(output);
            }
            output.start = *first;
            output.next = *first;
            output.truncated = true;
        }
        let mut bytes = Vec::new();
        File::open(directory.join(name(*first)))?.read_to_end(&mut bytes)?;
        let mut cursor = *first;
        for (channel, content) in parse(&bytes) {
            let length = u64::try_from(content.len()).unwrap_or(u64::MAX);
            let skip = usize::try_from(output.next.saturating_sub(cursor)).unwrap_or(usize::MAX);
            cursor += length;
            let Some(tail) = content.get(skip..).filter(|tail| !tail.is_empty()) else {
                continue;
            };
            let taken = tail.get(..budget.min(tail.len())).unwrap_or_default();
            if taken.is_empty() {
                return Ok(output);
            }
            budget -= taken.len();
            output.next += u64::try_from(taken.len()).unwrap_or(u64::MAX);
            output.records.push((channel, taken.to_vec()));
        }
    }
    Ok(output)
}
