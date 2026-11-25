//! Writer states that only a failing disk produces.

use std::{io::Write as _, path::PathBuf};

use ma2a_core::service::{Channel, LogWindow};

use super::{LogWriter, name, read, window};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

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
