//! Durable Process output: cursors, rotation, retention, truncation, and torn records.

#[path = "support/host.rs"]
mod support;

use std::io::Write as _;

use ma2a_core::service::{Channel, LogWindow};
use ma2a_services::supervisor::log::{self, LogWriter, MAX_RECORD_LEN, SEGMENT_BYTES};
use support::{TempRoot, TestResult, content};

const MIB: u64 = 1_048_576;

fn joined(read: &log::LogRead) -> Vec<u8> {
    read.records
        .iter()
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect()
}

#[test]
fn cursors_count_content_bytes_across_both_channels() -> TestResult {
    let root = TempRoot::new("log-cursor")?;
    let mut writer = LogWriter::create(root.path(), 4 * MIB)?;
    assert_eq!(writer.window(), LogWindow::new(0, 0));
    assert_eq!(writer.append(Channel::Output, b"hello ")?, 6);
    assert_eq!(writer.append(Channel::Error, b"oops")?, 10);
    assert_eq!(writer.append(Channel::Output, b"world")?, 15);
    writer.sync()?;

    assert_eq!(log::window(root.path())?, LogWindow::new(0, 15));
    let all = log::read(root.path(), 0, 1024)?;
    assert_eq!((all.start, all.truncated, all.next), (0, false, 15));
    assert_eq!(
        all.records,
        vec![
            (Channel::Output, b"hello ".to_vec()),
            (Channel::Error, b"oops".to_vec()),
            (Channel::Output, b"world".to_vec()),
        ]
    );

    // A cursor inside a record returns only its tail; the budget splits the next one.
    let middle = log::read(root.path(), 8, 4)?;
    assert_eq!((middle.start, middle.next), (8, 12));
    assert_eq!(
        middle.records,
        vec![
            (Channel::Error, b"ps".to_vec()),
            (Channel::Output, b"wo".to_vec()),
        ]
    );
    let end = log::read(root.path(), 15, 1024)?;
    assert!(end.records.is_empty());
    assert_eq!(end.next, 15);
    Ok(())
}

#[test]
fn retention_drops_whole_old_segments_and_reports_truncation() -> TestResult {
    let root = TempRoot::new("log-retention")?;
    let mut writer = LogWriter::create(root.path(), MIB)?;
    let chunk = content(usize::try_from(MAX_RECORD_LEN)?);
    let mut written = Vec::new();
    while u64::try_from(written.len())? < 4 * MIB {
        writer.append(Channel::Output, &chunk)?;
        written.extend_from_slice(&chunk);
    }
    let window = writer.window();
    let total = u64::try_from(written.len())?;
    assert_eq!(window.end, total);
    assert!(window.first_available > 0, "old output was dropped");
    // At most the limit plus the segment being written is kept.
    assert!(window.end - window.first_available <= MIB + SEGMENT_BYTES);
    assert_eq!(log::window(root.path())?, window);

    let old = log::read(root.path(), 0, 1 << 20)?;
    assert!(old.truncated);
    assert_eq!(old.start, window.first_available);
    let first = usize::try_from(window.first_available)?;
    let expected = written.get(first..first + joined(&old).len());
    assert_eq!(Some(joined(&old).as_slice()), expected);

    // Reading on from each batch returns the retained output exactly once.
    let mut cursor = old.next;
    let mut rest = joined(&old);
    while cursor < window.end {
        let batch = log::read(root.path(), cursor, 300_000)?;
        assert!(!batch.truncated);
        assert_eq!(batch.start, cursor);
        rest.extend(joined(&batch));
        cursor = batch.next;
    }
    assert_eq!(Some(rest.as_slice()), written.get(first..));
    Ok(())
}

#[test]
fn a_torn_final_record_is_invisible_until_complete() -> TestResult {
    let root = TempRoot::new("log-torn")?;
    let mut writer = LogWriter::create(root.path(), MIB)?;
    writer.append(Channel::Output, b"complete")?;
    writer.sync()?;
    drop(writer);
    let segment = root.path().join(format!("{:020}.seg", 0));
    let mut file = std::fs::OpenOptions::new().append(true).open(&segment)?;
    // A crash mid-write leaves a header promising more bytes than follow.
    file.write_all(&[1, 0, 0, 0, 9, b'p', b'a'])?;
    drop(file);

    assert_eq!(log::window(root.path())?, LogWindow::new(0, 8));
    let read = log::read(root.path(), 0, 1024)?;
    assert_eq!(read.records, vec![(Channel::Output, b"complete".to_vec())]);
    assert_eq!(read.next, 8);
    Ok(())
}

#[test]
fn invalid_records_and_reused_directories_are_refused() -> TestResult {
    let root = TempRoot::new("log-invalid")?;
    let mut writer = LogWriter::create(root.path(), MIB)?;
    assert!(writer.append(Channel::Output, b"").is_err());
    assert!(writer.append(Channel::Input, b"stdin").is_err());
    let oversized = vec![0; usize::try_from(MAX_RECORD_LEN)? + 1];
    assert!(writer.append(Channel::Error, &oversized).is_err());
    assert_eq!(writer.window(), LogWindow::new(0, 0));
    writer.append(Channel::Output, b"x")?;
    drop(writer);
    assert!(LogWriter::create(root.path(), MIB).is_err());
    Ok(())
}

#[test]
fn a_missing_log_has_an_empty_window() -> TestResult {
    let root = TempRoot::new("log-missing")?;
    let missing = root.path().join("never-created");
    assert_eq!(log::window(&missing)?, LogWindow::default());
    Ok(())
}

#[test]
fn a_missing_segment_is_reported_as_a_gap_never_read_across() -> TestResult {
    // Given: three segments, then the middle one gone while the first stayed, as after a
    // retention delete that failed for the oldest segment and succeeded for the next.
    let root = TempRoot::new("log-gap")?;
    let mut writer = LogWriter::create(root.path(), 16 * MIB)?;
    let chunk = vec![b'x'; usize::try_from(MAX_RECORD_LEN)?];
    while writer.window().end < 2 * SEGMENT_BYTES + 1 {
        writer.append(Channel::Output, &chunk)?;
    }
    writer.sync()?;
    let mut firsts: Vec<u64> = std::fs::read_dir(root.path())?
        .filter_map(|entry| {
            entry
                .ok()?
                .file_name()
                .to_str()?
                .strip_suffix(".seg")?
                .parse()
                .ok()
        })
        .collect();
    firsts.sort_unstable();
    let [first, second, third, ..] = firsts[..] else {
        return Err("expected three segments".into());
    };
    std::fs::remove_file(root.path().join(format!("{second:020}.seg")))?;

    // When
    let before = log::read(root.path(), first, usize::MAX)?;
    let after = log::read(root.path(), before.next, usize::MAX)?;

    // Then: the first read stops at the gap; the next one starts past it, marked truncated.
    assert_eq!(
        (before.start, before.truncated, before.next),
        (first, false, second)
    );
    assert_eq!((after.start, after.truncated), (third, true));
    Ok(())
}

/// Runs `write` while files may grow to at most `limit` bytes, so a write past it is short.
///
/// The limit is per process, and nextest runs each test in its own process. `SIGXFSZ` is
/// caught so the write fails with `EFBIG` instead of ending the process.
#[cfg(unix)]
fn with_file_size_limit<T>(limit: u64, write: impl FnOnce() -> T) -> TestResult<T> {
    use rustix::process::{Resource, Signal, getrlimit, setrlimit};
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _caught = runtime.block_on(async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::from_raw(
            Signal::XFSZ.as_raw(),
        ))
    })?;
    let previous = getrlimit(Resource::Fsize);
    let mut lowered = previous;
    lowered.current = Some(limit);
    setrlimit(Resource::Fsize, lowered)?;
    let result = write();
    setrlimit(Resource::Fsize, previous)?;
    Ok(result)
}

#[cfg(unix)]
#[test]
fn a_record_cut_short_by_a_failed_write_never_corrupts_the_next_one() -> TestResult {
    // Given: one record of 2 bytes, 7 file bytes with its header.
    let root = TempRoot::new("log-short")?;
    let mut writer = LogWriter::create(root.path(), MIB)?;
    assert_eq!(writer.append(Channel::Output, b"OK")?, 2);

    // When: the next record stops after its header and 3 of its bytes, as on a full disk.
    let failed = with_file_size_limit(15, || writer.append(Channel::Output, b"LOST-RECORD"))?;
    assert!(failed.is_err(), "{failed:?}");

    // And: a later record is written in full.
    assert_eq!(writer.append(Channel::Output, b"NEXT")?, 6);
    writer.sync()?;

    // Then: the live cursor, the window on disk, and the bytes read back agree.
    assert_eq!(writer.window(), LogWindow::new(0, 6));
    assert_eq!(log::window(root.path())?, LogWindow::new(0, 6));
    let read = log::read(root.path(), 0, 1024)?;
    assert_eq!(joined(&read), b"OKNEXT");
    assert_eq!(read.next, 6);
    Ok(())
}
