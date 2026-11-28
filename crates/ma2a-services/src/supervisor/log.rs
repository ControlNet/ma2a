//! Bounded, cursor-addressed durable output of one Process.
//!
//! Output is a sequence of records `channel u8 ‖ length u32 BE ‖ bytes` in segment files named
//! by the content cursor of their first byte. A cursor counts content bytes over both
//! channels in record order, so a reader that knows where it started knows every position.
//! Retention deletes whole oldest segments and never blocks the writer.

mod format;
mod read;

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
        let file = self
            .current
            .as_mut()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let mut record = Vec::with_capacity(HEADER + bytes.len());
        record.push(code);
        record.extend_from_slice(&length.to_be_bytes());
        record.extend_from_slice(bytes);
        if let Err(error) = file.write_all(&record) {
            // Roll back any partly written record, seal the segment, and account the dropped
            // chunk as a pending gap so the next segment or exit marker records a skip rather
            // than splicing bytes around the loss.
            let whole = self.segments.back().map_or(0, |last| last.file_bytes);
            let _cut = file
                .set_len(whole)
                .and_then(|()| file.seek(SeekFrom::Start(whole)));
            self.seal();
            self.discarded = self.discarded.saturating_add(u64::from(length));
            return Err(error);
        }
        let added = u64::from(length);
        if let Some(last) = self.segments.back_mut() {
            last.content += added;
            last.file_bytes += u64::try_from(record.len()).unwrap_or(u64::MAX);
        }
        self.end += added;
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
        let next_end = self.end.saturating_add(self.discarded);
        if next_end > self.end {
            let marker = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(self.directory.join(name(next_end)))?;
            self.segments.push_back(Segment::empty(next_end));
            self.end = next_end;
            self.discarded = 0;
            marker.sync_all()?;
        }
        self.discarded = 0;
        #[cfg(unix)]
        File::open(&self.directory)?.sync_all()?;
        Ok(())
    }

    fn retained_bytes(&self) -> u64 {
        self.segments.iter().map(|segment| segment.file_bytes).sum()
    }

    fn needs_rotate(&self) -> bool {
        self.current.is_none()
            || self
                .segments
                .back()
                .is_some_and(|last| last.file_bytes >= SEGMENT_BYTES)
    }

    fn retention_blocked(&self, record_bytes: u64) -> bool {
        if self.segments.len() <= 1 {
            return false;
        }
        let retained = self.retained_bytes();
        if self.discarded > 0 || self.needs_rotate() {
            retained > self.limit
        } else {
            retained.saturating_add(record_bytes) > self.limit.saturating_add(SEGMENT_BYTES)
        }
    }

    fn seal(&mut self) {
        if self.current.is_some() {
            let _synced = self.sync();
            self.current = None;
        }
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.sync()?;
        let next_end = self.end.saturating_add(self.discarded);
        // A segment still without a whole record lost its first one and could not be cut back:
        // it is this writer's own, at this cursor, and is started again empty.
        let again = self.discarded == 0
            && self
                .segments
                .back()
                .is_some_and(|last| last.content == 0 && last.first == next_end);
        // Written in place rather than appended: on Windows an append-only handle cannot be
        // truncated back after a failed write.
        let file = OpenOptions::new()
            .create_new(!again)
            .write(true)
            .truncate(again)
            .open(self.directory.join(name(next_end)))?;
        self.current = Some(file);
        self.end = next_end;
        self.discarded = 0;
        if !again {
            self.segments.push_back(Segment::empty(next_end));
        }
        Ok(())
    }

    /// Deletes oldest segments while their files hold more than the limit, record headers
    /// included, so the limit bounds the disk the log takes. A segment leaves the window only
    /// once its file is gone, so the window always matches the directory; one that cannot be
    /// deleted now stays first and is tried again after the next append.
    fn retain(&mut self) {
        let mut retained: u64 = self.segments.iter().map(|segment| segment.file_bytes).sum();
        while retained > self.limit && self.segments.len() > 1 {
            let Some(oldest) = self.segments.front().copied() else {
                break;
            };
            let path = self.directory.join(name(oldest.first));
            #[cfg(test)]
            let removed = if self.blocked_deletes.contains(&oldest.first) {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                fs::remove_file(&path)
            };
            #[cfg(not(test))]
            let removed = fs::remove_file(&path);
            match removed {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => break,
            }
            self.segments.pop_front();
            retained -= oldest.file_bytes;
        }
    }
}

#[cfg(test)]
mod tests;
