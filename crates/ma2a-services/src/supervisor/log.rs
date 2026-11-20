//! Bounded, cursor-addressed durable output of one Process.
//!
//! Output is a sequence of records `channel u8 ‖ length u32 BE ‖ bytes` in segment files named
//! by the content cursor of their first byte. A cursor counts content bytes over both
//! channels in record order, so a reader that knows where it started knows every position.
//! Retention deletes whole oldest segments and never blocks the writer.

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
};

use ma2a_core::service::{Channel, LogWindow};

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
        file.write_all(&record)?;
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
        let file = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(self.directory.join(name(self.end)))?;
        self.current = Some(file);
        self.segments.push_back(Segment {
            first: self.end,
            content: 0,
            file_bytes: 0,
        });
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
    let firsts = match segments(directory) {
        Ok(firsts) => firsts,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(LogWindow::default()),
        Err(error) => return Err(error),
    };
    let (Some(first), Some(last)) = (firsts.first(), firsts.last()) else {
        return Ok(LogWindow::default());
    };
    let bytes = fs::read(directory.join(name(*last)))?;
    Ok(LogWindow::new(*first, last + content_len(&parse(&bytes))))
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
