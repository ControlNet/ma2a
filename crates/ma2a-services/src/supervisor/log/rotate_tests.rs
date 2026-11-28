//! Reader visibility across segment rotation and rollback.

use std::sync::{Arc, Mutex};

use ma2a_core::service::{Channel, LogFrom, LogWindow};
use rustix::process::{Resource, Signal, getrlimit, setrlimit};

use super::{LogWriter, read, set_after_rotate, window};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

struct TempDirectory(std::path::PathBuf);

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

fn with_file_size_limit<T>(limit: u64, write: impl FnOnce() -> T) -> Result<T, std::io::Error> {
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

#[test]
fn reader_between_rotation_and_failed_first_write_never_sees_invalid_cursor() -> TestResult {
    let directory = TempDirectory::new("rotate-visibility")?;
    let mut writer = LogWriter::create(&directory.0, 1_048_576)?;

    // Fail once on an empty log so the next append rotates at cursor 4.
    let first_fail = with_file_size_limit(4, || writer.append(Channel::Output, b"aaaa"))?;
    assert!(first_fail.is_err());

    let seen_empty = Arc::new(Mutex::new(None));
    let seen_empty_hook = Arc::clone(&seen_empty);
    let dir_empty = directory.0.clone();
    set_after_rotate(Box::new(move || {
        *seen_empty_hook.lock().unwrap() = Some(window(&dir_empty));
    }));

    let second_fail = with_file_size_limit(4, || writer.append(Channel::Output, b"bbbb"))?;
    assert!(second_fail.is_err());

    let seen = seen_empty
        .lock()
        .unwrap()
        .take()
        .ok_or("after_rotate did not run")??;
    let after = window(&directory.0)?;
    assert!(
        after.resolve(LogFrom::Cursor(seen.end)).is_some(),
        "cursor {} seen during rotation became invalid in {after:?}",
        seen.end
    );
    assert_eq!(seen, LogWindow::new(0, 0));

    // Also verify after a committed segment: a rotated segment stays invisible until its first
    // record completes, while finish() publishes the end-of-log gap marker.
    let populated_dir = TempDirectory::new("rotate-visibility-populated")?;
    let mut populated = LogWriter::create(&populated_dir.0, 1_048_576)?;
    assert_eq!(populated.append(Channel::Output, b"head")?, 4);
    populated.sync()?;
    let lost_first = with_file_size_limit(14, || populated.append(Channel::Output, b"lost"))?;
    assert!(lost_first.is_err());

    let seen_populated = Arc::new(Mutex::new(None));
    let seen_pop_hook = Arc::clone(&seen_populated);
    let dir_pop = populated_dir.0.clone();
    set_after_rotate(Box::new(move || {
        *seen_pop_hook.lock().unwrap() = Some(window(&dir_pop));
    }));
    let lost_second = with_file_size_limit(4, || populated.append(Channel::Output, b"more"))?;
    assert!(lost_second.is_err());

    let seen_pop = seen_populated
        .lock()
        .unwrap()
        .take()
        .ok_or("after_rotate did not run for populated log")??;
    let after_pop = window(&populated_dir.0)?;
    assert_eq!(seen_pop, LogWindow::new(0, 4));
    assert!(after_pop.resolve(LogFrom::Cursor(seen_pop.end)).is_some());

    populated.finish()?;
    assert_eq!(window(&populated_dir.0)?, LogWindow::new(0, 12));
    let tail = read(&populated_dir.0, 4, 1024)?;
    assert_eq!((tail.start, tail.truncated, tail.next), (12, true, 12));
    assert!(tail.records.is_empty());
    Ok(())
}
