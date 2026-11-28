//! Writer states that only a failing disk produces.

use std::{io::Write as _, path::PathBuf};

use ma2a_core::service::{Channel, LogWindow};

use super::{LogWriter, MAX_RECORD_LEN, SEGMENT_BYTES, name, read, window};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

const MIB: u64 = 1_048_576;

fn disk_bytes(directory: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(directory)? {
        total += entry?.metadata()?.len();
    }
    Ok(total)
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new(label: &str) -> std::io::Result<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce).map_err(std::io::Error::other)?;
        let name = format!("ma2a-log-{label}-{:016x}", u64::from_le_bytes(nonce));
        Ok(Self(std::env::temp_dir().join(name)))
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_first_record_that_could_not_be_cut_back_never_blocks_later_ones() -> TestResult {
    // Given: a new segment whose first record failed half-written, and whose cut back to empty
    // failed too, so the writer let go of it.
    let directory = TempDirectory::new("first-torn")?;
    let mut writer = LogWriter::create(&directory.0, 1_048_576)?;
    writer.rotate()?;
    let mut torn = std::fs::OpenOptions::new()
        .append(true)
        .open(directory.0.join(name(0)))?;
    torn.write_all(&[1, 0, 0, 0, 9, b'L', b'O'])?;
    writer.current = None;

    // When: the disk recovers and the next record is written.
    assert_eq!(writer.append(Channel::Output, b"NEXT")?, 4);
    writer.sync()?;

    // Then: it is readable once, at the cursor where the failed record would have started.
    assert_eq!(writer.window(), LogWindow::new(0, 4));
    assert_eq!(window(&directory.0)?, LogWindow::new(0, 4));
    let read = read(&directory.0, 0, 1024)?;
    assert_eq!(read.records, vec![(Channel::Output, b"NEXT".to_vec())]);
    assert_eq!(read.next, 4);
    Ok(())
}

fn write_blocked_past_limit(
    directory: &std::path::Path,
    limit: u64,
) -> Result<(LogWriter, u64, u64), Box<dyn std::error::Error + Send + Sync>> {
    let mut writer = LogWriter::create(directory, limit)?;
    writer.blocked_deletes.insert(0);
    let chunk = vec![b'a'; usize::try_from(MAX_RECORD_LEN)?];
    let mut produced = 0_u64;
    while produced < 5 * MIB {
        writer.append(Channel::Output, &chunk)?;
        produced += u64::try_from(chunk.len())?;
    }
    writer.sync()?;
    let stalled_end = writer.window().end;
    assert!(stalled_end < produced, "excess output was discarded");
    Ok((writer, stalled_end, produced))
}

#[test]
fn blocked_retention_bounds_disk_and_records_gap_on_resume() -> TestResult {
    // Given: a 2 MiB log whose oldest segment cannot be deleted (simulating a Windows reader
    // holding it open without delete sharing), with 5 MiB appended.
    let limit = 2 * MIB;
    let directory = TempDirectory::new("blocked-retention")?;
    let (mut writer, stalled_end, produced) = write_blocked_past_limit(&directory.0, limit)?;

    // Then: disk usage stays within log_limit plus one segment, and window matches disk.
    let on_disk = disk_bytes(&directory.0)?;
    assert!(
        on_disk <= limit + SEGMENT_BYTES,
        "expected <= {} bytes on disk, got {on_disk}",
        limit + SEGMENT_BYTES
    );
    assert_eq!(window(&directory.0)?, writer.window());

    // When: segment 0 becomes deletable and new output arrives.
    writer.blocked_deletes.remove(&0);
    let resumed_end = writer.append(Channel::Output, b"RESUMED")?;
    writer.sync()?;

    // Then: retention reclaims old segments, the discarded span is a cursor gap, and reading
    // across it reports truncation then delivers the resumed output.
    assert!(resumed_end > produced, "gap kept discarded byte offset");
    assert_eq!(window(&directory.0)?, writer.window());
    let on_disk_after = disk_bytes(&directory.0)?;
    assert!(
        on_disk_after <= limit + SEGMENT_BYTES,
        "after resume: {on_disk_after} bytes"
    );
    let before = read(&directory.0, writer.window().first_available, usize::MAX)?;
    assert_eq!(before.next, stalled_end);
    let after = read(&directory.0, before.next, usize::MAX)?;
    assert!(after.truncated);
    assert_eq!(after.start, produced);
    assert_eq!(after.records, vec![(Channel::Output, b"RESUMED".to_vec())]);
    assert_eq!(after.next, produced + 7);
    Ok(())
}

#[test]
fn blocked_retention_records_discarded_tail_at_exit() -> TestResult {
    for release_before_finish in [false, true] {
        let limit = 2 * MIB;
        let directory = TempDirectory::new("blocked-exit")?;
        let (mut writer, stalled_end, produced) = write_blocked_past_limit(&directory.0, limit)?;
        if release_before_finish {
            writer.blocked_deletes.remove(&0);
        }

        // When: the writer finishes with no further append.
        writer.finish()?;
        writer.finish()?;

        // Then: the gap is durable within the disk bound and readable as a truncated tail.
        let on_disk = disk_bytes(&directory.0)?;
        assert!(
            on_disk <= limit + SEGMENT_BYTES,
            "expected <= {} bytes on disk, got {on_disk}",
            limit + SEGMENT_BYTES
        );
        assert_eq!(writer.window().end, produced);
        assert_eq!(window(&directory.0)?, writer.window());
        let tail = read(&directory.0, stalled_end, usize::MAX)?;
        assert!(tail.truncated);
        assert_eq!((tail.start, tail.next), (produced, produced));
        assert!(tail.records.is_empty());
    }
    Ok(())
}

#[cfg(windows)]
#[test]
fn windows_unshared_reader_on_oldest_segment_bounds_disk_and_recovers() -> TestResult {
    use std::os::windows::fs::OpenOptionsExt as _;

    const FILE_SHARE_READ: u32 = 1;

    // Given: a 2 MiB log whose first segment has rotated and is held open without delete sharing.
    let limit = 2 * MIB;
    let directory = TempDirectory::new("win-blocked-retention")?;
    let mut writer = LogWriter::create(&directory.0, limit)?;
    let chunk = vec![b'w'; usize::try_from(MAX_RECORD_LEN)?];
    while writer.segments.len() < 2 {
        writer.append(Channel::Output, &chunk)?;
    }
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(directory.0.join(name(0)))?;

    // When: enough output is written to force multiple rotations past log_limit.
    let mut produced = writer.window().end;
    while produced < 5 * MIB {
        writer.append(Channel::Output, &chunk)?;
        produced += u64::try_from(chunk.len())?;
    }
    writer.sync()?;
    let on_disk = disk_bytes(&directory.0)?;
    assert!(
        on_disk <= limit + SEGMENT_BYTES,
        "expected <= {} bytes on disk, got {on_disk}",
        limit + SEGMENT_BYTES
    );
    let stalled_end = writer.window().end;
    assert!(stalled_end < produced);

    // And: the reader closes the oldest segment and output resumes.
    drop(held);
    let resumed_end = writer.append(Channel::Output, b"RECOVERED")?;
    writer.sync()?;
    assert_eq!(resumed_end, produced + 9);
    assert!(disk_bytes(&directory.0)? <= limit + SEGMENT_BYTES);
    let before = read(&directory.0, writer.window().first_available, usize::MAX)?;
    let after = read(&directory.0, before.next, usize::MAX)?;
    assert!(after.truncated);
    assert_eq!(after.start, produced);
    assert_eq!(
        after.records,
        vec![(Channel::Output, b"RECOVERED".to_vec())]
    );
    Ok(())
}
