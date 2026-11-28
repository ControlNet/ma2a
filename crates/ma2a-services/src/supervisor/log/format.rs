//! Segment file naming and record framing helpers.

use std::{fs, io, path::Path};

use ma2a_core::service::Channel;

/// Size of one record header: `channel u8 || length u32 BE`.
pub(super) const HEADER: usize = 5;

pub(super) fn name(first: u64) -> String {
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
pub(super) fn segments(directory: &Path) -> io::Result<Vec<u64>> {
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
pub(super) fn parse(bytes: &[u8]) -> Vec<(Channel, &[u8])> {
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

pub(super) fn content_len(records: &[(Channel, &[u8])]) -> u64 {
    records
        .iter()
        .map(|(_, bytes)| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .sum()
}
