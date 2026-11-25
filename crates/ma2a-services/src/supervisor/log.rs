//! Bounded, cursor-addressed durable output of one Process.
//!
//! Output is a sequence of records `channel u8 ‖ length u32 BE ‖ bytes` in segment files named
//! by the content cursor of their first byte. A cursor counts content bytes over both
//! channels in record order, so a reader that knows where it started knows every position.
//! Retention deletes whole oldest segments and never blocks the writer.

mod read;

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

use ma2a_core::service::{Channel, LogWindow};

pub use read::{LogRead, read, window};

/// A segment is closed and synced once it holds this many file bytes.
pub const SEGMENT_BYTES: u64 = 1_048_576;
/// Largest content of one record: one data frame.
pub const MAX_RECORD_LEN: u32 = 65_535;
const HEADER: usize = 5;

/// One retained output chunk.
pub type LogRecord = (Channel, Vec<u8>);

#[derive(Clone, Copy, Debug)]
struct Segment {
    first: u64,
    content: u64,
    file_bytes: u64,
}

fn name(first: u64) -> String {
    format!("{first:020}.seg")
}

const fn channel(code: u8) -> Option<Channel> {
    match code {
        1 => Some(Channel::Output),
        2 => Some(Channel::Error),
        _ => None,
    }
}

/// Lists segment first cursors in ascending order.
fn segments(directory: &Path) -> io::Result<Vec<u64>> {
    let mut firsts = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(stem) = file_name
            .to_str()
            .and_then(|text| text.strip_suffix(".seg"))
        else {
            continue;
        };
        if stem.len() == 20
            && stem.bytes().all(|byte| byte.is_ascii_digit())
            && let Ok(first) = stem.parse()
        {
            firsts.push(first);
        }
    }
    firsts.sort_unstable();
    Ok(firsts)
}

/// Parses complete records of one segment; a torn final record is ignored.
fn parse(bytes: &[u8]) -> Vec<(Channel, &[u8])> {
    let mut records = Vec::new();
    let mut rest = bytes;
    while let Some((header, body)) = rest.split_first_chunk::<HEADER>() {
        let [code, length @ ..] = *header;
        let length = usize::try_from(u32::from_be_bytes(length)).unwrap_or(usize::MAX);
        let (Some(channel), Some(content)) = (channel(code), body.get(..length)) else {
            break;
        };
        records.push((channel, content));
        rest = body.get(length..).unwrap_or_default();
    }
    records
}

fn content_len(records: &[(Channel, &[u8])]) -> u64 {
    records
        .iter()
        .map(|(_, bytes)| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .sum()
}

/// Appends output and enforces the retention bound.
#[derive(Debug)]
pub struct LogWriter {
    directory: PathBuf,
    limit: u64,
    segments: VecDeque<Segment>,
    current: Option<File>,
    end: u64,
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
        if self.current.is_none()
            || self
                .segments
                .back()
                .is_some_and(|last| last.file_bytes >= SEGMENT_BYTES)
        {
            self.rotate()?;
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
            // A partly written record would shift every later one, so the segment is cut back
            // to its last whole record. If that fails, the next append starts a new segment
            // and the torn record stays last in this one, where reads ignore it; a segment
            // with no whole record is started again instead.
            let whole = self.segments.back().map_or(0, |last| last.file_bytes);
            let cut = file
                .set_len(whole)
                .and_then(|()| file.seek(SeekFrom::Start(whole)));
            if cut.is_err() {
                self.current = None;
            }
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

    fn rotate(&mut self) -> io::Result<()> {
        self.sync()?;
        // A segment still without a whole record lost its first one and could not be cut back:
        // it is this writer's own, at this cursor, and is started again empty.
        let again = self.segments.back().is_some_and(|last| last.content == 0);
        // Written in place rather than appended: on Windows an append-only handle cannot be
        // truncated back after a failed write.
        let file = OpenOptions::new()
            .create_new(!again)
            .write(true)
            .truncate(again)
            .open(self.directory.join(name(self.end)))?;
        self.current = Some(file);
        if !again {
            self.segments.push_back(Segment {
                first: self.end,
                content: 0,
                file_bytes: 0,
            });
        }
        Ok(())
    }

    /// Deletes oldest segments while over the limit. A segment leaves the window only once its
    /// file is gone, so the window always matches the directory; one that cannot be deleted
    /// now stays first and is tried again after the next append.
    fn retain(&mut self) {
        let mut retained: u64 = self.segments.iter().map(|segment| segment.content).sum();
        while retained > self.limit && self.segments.len() > 1 {
            let Some(oldest) = self.segments.front().copied() else {
                break;
            };
            match fs::remove_file(self.directory.join(name(oldest.first))) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => break,
            }
            self.segments.pop_front();
            retained -= oldest.content;
        }
    }
}

#[cfg(test)]
mod tests;
