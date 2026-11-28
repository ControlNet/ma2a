//! Retention and staged segment cleanup for [`LogWriter`].

use std::{fs, io};

use super::{LogWriter, format::name};

pub(super) fn staging_name(first: u64) -> String {
    format!("{first:020}.part")
}

impl LogWriter {
    pub(super) fn usage(&self) -> (u64, usize) {
        self.segments
            .iter()
            .fold((0, 0), |(bytes, count), segment| {
                (
                    bytes + segment.file_bytes,
                    count + usize::from(segment.file_bytes > 0),
                )
            })
    }

    pub(super) fn needs_rotate(&self) -> bool {
        self.current.is_none()
            || self
                .segments
                .back()
                .is_some_and(|last| last.file_bytes >= super::SEGMENT_BYTES)
    }

    pub(super) fn retention_blocked(&self, record_bytes: u64) -> bool {
        let (retained, non_empty) = self.usage();
        if non_empty <= 1 {
            return false;
        }
        if self.discarded > 0 || self.needs_rotate() {
            retained > self.limit
        } else {
            retained.saturating_add(record_bytes) > self.limit.saturating_add(super::SEGMENT_BYTES)
        }
    }

    pub(super) fn remove_file(
        &self,
        file_name: &str,
        #[cfg_attr(
            not(test),
            expect(unused_variables, reason = "used by test delete blocking")
        )]
        key: u64,
    ) -> io::Result<()> {
        #[cfg(test)]
        if self.blocked_deletes.contains(&key) {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        match fs::remove_file(self.directory.join(file_name)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    pub(super) fn drop_staged(&mut self) -> io::Result<()> {
        let Some(first) = self.staged else {
            return Ok(());
        };
        self.remove_file(&staging_name(first), first)?;
        self.staged = None;
        Ok(())
    }

    /// Deletes oldest segments while their files hold more than the limit, record headers
    /// included, so the limit bounds the disk the log takes. A segment leaves the window only
    /// once its file is gone, so the window always matches the directory; one that cannot be
    /// deleted now stays first and is tried again after the next append.
    pub(super) fn retain(&mut self) {
        let (mut retained, mut non_empty) = self.usage();
        while retained > self.limit && self.segments.len() > 1 {
            let Some(oldest) = self.segments.front().copied() else {
                break;
            };
            if oldest.file_bytes > 0 && non_empty <= 1 {
                break;
            }
            if self.remove_file(&name(oldest.first), oldest.first).is_err() {
                break;
            }
            self.segments.pop_front();
            retained -= oldest.file_bytes;
            if oldest.file_bytes > 0 {
                non_empty -= 1;
            }
        }
    }
}
