//! One frozen snapshot may span ordered frames; no collection is silently truncated.
use serde::{Deserialize, Serialize};

use super::{ApiError, MAX_LOCAL_RESPONSE_BYTES, encode_hex};

pub(crate) const SNAPSHOT_FRAGMENT_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FragmentType {
    SnapshotFragment,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotFragment {
    #[serde(rename = "type")]
    kind: FragmentType,
    revision: u64,
    runtime_boot_id: String,
    index: u64,
    last: bool,
    data_hex: String,
}

/// Owns one immutable serialized snapshot, yielding bounded wire fragments lazily.
pub(crate) struct SnapshotFragments {
    bytes: Vec<u8>,
    revision: u64,
    boot_id: String,
    offset: usize,
}

impl SnapshotFragments {
    pub(crate) const fn new(bytes: Vec<u8>, revision: u64, boot_id: String) -> Self {
        Self {
            bytes,
            revision,
            boot_id,
            offset: 0,
        }
    }
}

impl Iterator for SnapshotFragments {
    type Item = Result<Vec<u8>, ApiError>;

    fn next(&mut self) -> Option<Self::Item> {
        let remaining = self.bytes.get(self.offset..)?;
        if remaining.is_empty() {
            return None;
        }
        let chunk = remaining.get(..remaining.len().min(SNAPSHOT_FRAGMENT_BYTES))?;
        let fragment = SnapshotFragment {
            kind: FragmentType::SnapshotFragment,
            revision: self.revision,
            runtime_boot_id: self.boot_id.clone(),
            index: (self.offset / SNAPSHOT_FRAGMENT_BYTES) as u64,
            last: chunk.len() == remaining.len(),
            data_hex: encode_hex(chunk),
        };
        self.offset += chunk.len();
        Some(
            serde_json::to_vec(&fragment)
                .map_err(|_| ApiError::invalid_input())
                .and_then(|encoded| {
                    if encoded.len() <= MAX_LOCAL_RESPONSE_BYTES {
                        Ok(encoded)
                    } else {
                        Err(ApiError::invalid_input())
                    }
                }),
        )
    }
}

/// Incomplete snapshots never become authoritative client state.
#[derive(Default)]
pub(crate) struct SnapshotAssembly {
    stamp: Option<(u64, String)>,
    next_index: u64,
    bytes: Vec<u8>,
    complete: bool,
}

impl SnapshotAssembly {
    pub(crate) fn push(&mut self, encoded: &[u8]) -> Result<bool, ApiError> {
        if encoded.len() > MAX_LOCAL_RESPONSE_BYTES {
            return Err(ApiError::invalid_input());
        }
        let fragment: SnapshotFragment =
            serde_json::from_slice(encoded).map_err(|_| ApiError::invalid_input())?;
        let stamp = (fragment.revision, fragment.runtime_boot_id.clone());
        if self.complete
            || fragment.index != self.next_index
            || self
                .stamp
                .as_ref()
                .is_some_and(|expected| expected != &stamp)
            || fragment.runtime_boot_id.len() != 32
            || !fragment.runtime_boot_id.bytes().all(is_hex)
            || fragment.data_hex.is_empty()
            || fragment.data_hex.len() > SNAPSHOT_FRAGMENT_BYTES * 2
            || !fragment.data_hex.len().is_multiple_of(2)
            || (!fragment.last && fragment.data_hex.len() != SNAPSHOT_FRAGMENT_BYTES * 2)
            || !fragment.data_hex.bytes().all(is_hex)
        {
            return Err(ApiError::invalid_input());
        }
        for pair in fragment.data_hex.as_bytes().chunks_exact(2) {
            let [high, low] = pair else {
                return Err(ApiError::invalid_input());
            };
            self.bytes.push(nibble(*high) * 16 + nibble(*low));
        }
        self.stamp = Some(stamp);
        self.next_index = self
            .next_index
            .checked_add(1)
            .ok_or_else(ApiError::invalid_input)?;
        self.complete = fragment.last;
        Ok(self.complete)
    }

    pub(crate) fn finish(self) -> Result<(Vec<u8>, u64, String), ApiError> {
        let (revision, boot) = self
            .stamp
            .filter(|_| self.complete)
            .ok_or_else(ApiError::invalid_input)?;
        Ok((self.bytes, revision, boot))
    }
}

const fn is_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (byte >= b'a' && byte <= b'f')
}

const fn nibble(byte: u8) -> u8 {
    if byte <= b'9' {
        byte - b'0'
    } else {
        byte - b'a' + 10
    }
}

#[cfg(test)]
mod tests;
