//! Bounded, cursor-addressed durable output of one Process.
//!
//! Output is a sequence of records `channel u8 ‖ length u32 BE ‖ bytes` in segment files named
//! by the content cursor of their first byte. A cursor counts content bytes over both
//! channels in record order, so a reader that knows where it started knows every position.
//! Retention deletes whole oldest segments and never blocks the writer.

mod format;
mod read;
mod retain;

#[cfg(test)]
use std::collections::BTreeSet;
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use format::{HEADER, content_len, name, parse, segments};
use ma2a_core::service::{Channel, LogWindow};
use retain::staging_name;

pub use read::{LogRead, read, window};

/// A segment is closed and synced once it holds this many file bytes.
pub const SEGMENT_BYTES: u64 = 1_048_576;
/// Largest content of one record: one data frame.
pub const MAX_RECORD_LEN: u32 = 65_535;

/// One retained output chunk.
pub type LogRecord = (Channel, Vec<u8>);

#[derive(Clone, Copy, Debug)]
struct Segment {
    first: u64,
    content: u64,
    file_bytes: u64,
}

impl Segment {
    const fn empty(first: u64) -> Self {
        Self {
            first,
            content: 0,
            file_bytes: 0,
        }
    }
}

/// Appends output and enforces the retention bound.
#[derive(Debug)]
pub struct LogWriter {
    directory: PathBuf,
    limit: u64,
    segments: VecDeque<Segment>,
    current: Option<File>,
    staged: Option<u64>,
    end: u64,
    discarded: u64,
    #[cfg(test)]
    pub(super) blocked_deletes: BTreeSet<u64>,
}

impl LogWriter {
    /// Creates an empty log in a new or empty directory.
    ///
    /// # Errors
    /// Returns an I/O error when the directory cannot be created or already holds output.
    pub fn create(directory: &Path, limit: u64) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        if !segments(directory)?.is_empty() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        Ok(Self {
            directory: directory.to_path_buf(),
            limit,
            segments: VecDeque::new(),
            current: None,
            staged: None,
            end: 0,
            discarded: 0,
            #[cfg(test)]
            blocked_deletes: BTreeSet::new(),
        })
    }

    /// Returns the retained window.
    pub fn window(&self) -> LogWindow {
        let first = self
            .segments
            .front()
            .map_or(self.end, |segment| segment.first);
        LogWindow::new(first, self.end)
    }

    /// Appends one record and returns the cursor after it.
    ///
    /// # Errors
    /// Returns an I/O error when the record cannot be written; `InvalidInput` for an empty or
    /// oversized record or the input channel.
    pub fn append(&mut self, channel: Channel, bytes: &[u8]) -> io::Result<u64> {
        let code = match channel {
            Channel::Output => 1,
            Channel::Error => 2,
            Channel::Input => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        };
        let length = u32::try_from(bytes.len())
            .ok()
            .filter(|length| (1..=MAX_RECORD_LEN).contains(length))
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let record_len = u64::try_from(HEADER + bytes.len()).unwrap_or(u64::MAX);
        if self.discarded > 0 || self.needs_rotate() || self.retention_blocked(record_len) {
            self.retain();
            if self.retention_blocked(record_len) {
                // Close the open segment before discarding so resumed output starts a new
                // segment past the discarded cursor gap rather than appending across it.
                self.seal();
                self.discarded = self.discarded.saturating_add(u64::from(length));
                return Ok(self.end);
            }
        }
        if self.needs_rotate()
            && let Err(error) = self.rotate()
        {
            self.seal();
            self.discarded = self.discarded.saturating_add(u64::from(length));
            return Err(error);
        }
        let mut record = Vec::with_capacity(HEADER + bytes.len());
        record.push(code);
        record.extend_from_slice(&length.to_be_bytes());
        record.extend_from_slice(bytes);
        let write_result = self
            .current
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?
            .write_all(&record);
        if let Err(error) = write_result {
            self.rollback_append(u64::from(length));
            return Err(error);
        }
        let added = u64::from(length);
        let file_bytes = u64::try_from(record.len()).unwrap_or(u64::MAX);
        if let Some(first) = self.staged {
            if let Err(error) = fs::rename(
                self.directory.join(staging_name(first)),
                self.directory.join(name(first)),
            ) {
                self.rollback_append(added);
                return Err(error);
            }
            self.staged = None;
            self.discarded = 0;
            self.end = first + added;
            self.segments.push_back(Segment {
                first,
                content: added,
                file_bytes,
            });
        } else {
            if let Some(last) = self.segments.back_mut() {
                last.content += added;
                last.file_bytes += file_bytes;
            }
            self.end += added;
        }
        self.retain();
        Ok(self.end)
    }

    /// Makes every written record durable.
    ///
    /// # Errors
    /// Returns an I/O error when the current segment cannot be synced.
    pub fn sync(&mut self) -> io::Result<()> {
        self.current.as_ref().map_or(Ok(()), File::sync_all)
    }

    /// Makes every written record durable and persists any unrecovered retention gap at exit.
    ///
    /// # Errors
    /// Returns an I/O error when the current segment cannot be synced or the gap marker cannot
    /// be created.
    pub fn finish(&mut self) -> io::Result<()> {
        self.sync()?;
        self.retain();
        if self.staged.is_some() {
            self.seal();
            let _dropped = self.drop_staged();
        }
        let next_end = self.end.saturating_add(self.discarded);
        if next_end > self.end {
            let marker = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.directory.join(name(next_end)))?;
            if let Err(error) = marker.sync_all() {
                drop(marker);
                let _removed = self.remove_file(&name(next_end), next_end);
                return Err(error);
            }
            self.segments.push_back(Segment::empty(next_end));
            self.end = next_end;
            self.discarded = 0;
        }
        self.discarded = 0;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    fn seal(&mut self) {
        if self.current.is_some() {
            let _synced = self.sync();
            self.current = None;
        }
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.sync()?;
        if self.discarded > 0 {
            self.drop_staged()?;
        }
        let next_end = self.end.saturating_add(self.discarded);
        // A staged segment that lost its first record and could not be removed at this cursor
        // is truncated and reused in place.
        let again = self.discarded == 0 && self.staged == Some(next_end);
        // Written in place rather than appended: on Windows an append-only handle cannot be
        // truncated back after a failed write.
        let file = OpenOptions::new()
            .create_new(!again)
            .write(true)
            .truncate(again)
            .open(self.directory.join(staging_name(next_end)))?;
        self.current = Some(file);
        self.staged = Some(next_end);
        #[cfg(all(test, unix))]
        run_after_rotate();
        Ok(())
    }

    fn rollback_append(&mut self, length: u64) {
        // Roll back any partly written record, seal the segment, and account the dropped chunk
        // as a pending gap so the next segment or exit marker records a skip.
        let whole = if self.staged.is_some() {
            0
        } else {
            self.segments.back().map_or(0, |last| last.file_bytes)
        };
        if let Some(file) = self.current.as_mut() {
            let _cut = file
                .set_len(whole)
                .and_then(|()| file.seek(SeekFrom::Start(whole)));
        }
        self.seal();
        if self.staged.is_some() {
            let _removed = self.drop_staged();
        }
        self.discarded = self.discarded.saturating_add(length);
    }
}

#[cfg(all(test, unix))]
thread_local! {
    static AFTER_ROTATE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, unix))]
fn set_after_rotate(hook: Box<dyn FnOnce()>) {
    AFTER_ROTATE.with(|slot| *slot.borrow_mut() = Some(hook));
}

#[cfg(all(test, unix))]
fn run_after_rotate() {
    let hook = AFTER_ROTATE.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

#[cfg(all(test, unix))]
mod rotate_tests;
#[cfg(test)]
mod tests;
