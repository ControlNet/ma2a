//! In-memory frame pipes and temporary roots for host tests.

use std::path::{Path, PathBuf};

use ma2a_services::{Frame, FrameReader, FrameWriter};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Server-side frame halves handed to a host under test.
pub(crate) type ServerIo = (
    FrameReader<ReadHalf<DuplexStream>>,
    FrameWriter<WriteHalf<DuplexStream>>,
);

/// Client-side frame halves driven by the test.
pub(crate) type ClientIo = (
    FrameReader<ReadHalf<DuplexStream>>,
    FrameWriter<WriteHalf<DuplexStream>>,
);

/// Returns connected client and server frame halves.
#[allow(dead_code, reason = "shared by only some host test binaries")]
pub(crate) fn pipe() -> (ClientIo, ServerIo) {
    pipe_with(1 << 20)
}

/// Returns connected halves whose in-flight bytes are bounded by `capacity`.
pub(crate) fn pipe_with(capacity: usize) -> (ClientIo, ServerIo) {
    let (client, server) = tokio::io::duplex(capacity);
    let (client_read, client_write) = tokio::io::split(client);
    let (server_read, server_write) = tokio::io::split(server);
    (
        (
            FrameReader::new(client_read),
            FrameWriter::new(client_write),
        ),
        (
            FrameReader::new(server_read),
            FrameWriter::new(server_write),
        ),
    )
}

/// Reads every frame until the server half-closes.
pub(crate) async fn drain(
    reader: &mut FrameReader<ReadHalf<DuplexStream>>,
) -> TestResult<Vec<Frame>> {
    let mut frames = Vec::new();
    while let Some(frame) = reader.next().await? {
        frames.push(frame);
    }
    Ok(frames)
}

/// A uniquely named directory removed on drop.
#[derive(Debug)]
pub(crate) struct TempRoot(PathBuf);

impl TempRoot {
    pub(crate) fn new(label: &str) -> TestResult<Self> {
        let mut nonce = [0_u8; 8];
        getrandom::fill(&mut nonce)?;
        let name = format!("ma2a-services-{label}-{:016x}", u64::from_le_bytes(nonce));
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir(&path)?;
        Ok(Self(path.canonicalize()?))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _removed = std::fs::remove_dir_all(&self.0);
    }
}

/// Deterministic pseudo-random content of `length` bytes.
#[allow(dead_code, reason = "shared by only some host test binaries")]
pub(crate) fn content(length: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state.to_le_bytes()[0]
        })
        .collect()
}
