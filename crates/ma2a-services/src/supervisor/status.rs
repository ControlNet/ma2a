//! The supervisor's durable status record: what it last knew about its child.
//!
//! The record is replaced atomically (temporary file, sync, rename), so a reader sees either
//! the previous status or the next one. It is the only evidence left once the supervisor has
//! exited, and it is never taken as proof that a child is still running.

use std::{fs, io, io::Write as _, path::Path};

use ma2a_core::service::Termination;

const MAGIC: &[u8; 7] = b"MA2AST1";
const FILE: &str = "status";
const TEMPORARY: &str = "status.tmp";

/// How a supervised child ended.
#[expect(
    clippy::exhaustive_enums,
    reason = "end causes are a closed persisted vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndCause {
    /// It ended by itself.
    Exited,
    /// A terminate request ended it.
    Terminated,
}

/// What a supervisor last recorded.
#[expect(
    clippy::exhaustive_enums,
    reason = "supervisor statuses are a closed persisted vocabulary"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupervisorStatus {
    /// Waiting for `go`; no child exists.
    Awaiting,
    /// The child was spawned with this process ID at this time.
    Running(u32, u64),
    /// The child ended.
    Ended(Termination, EndCause, u64),
    /// The program could not be started.
    SpawnFailed(u64),
    /// No `go` arrived in time, or `abort` was requested; no child ever existed.
    NeverStarted(u64),
}

impl SupervisorStatus {
    /// Returns whether no further status can follow.
    pub const fn is_final(self) -> bool {
        !matches!(self, Self::Awaiting | Self::Running(..))
    }

    /// Encodes the record.
    pub fn to_bytes(self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        match self {
            Self::Awaiting => bytes.push(0),
            Self::Running(pid, started_ms) => {
                bytes.push(1);
                bytes.extend_from_slice(&pid.to_be_bytes());
                bytes.extend_from_slice(&started_ms.to_be_bytes());
            }
            Self::Ended(termination, cause, ended_ms) => {
                bytes.push(2);
                let (kind, value) = match termination {
                    Termination::Exited(code) => (0, code),
                    Termination::Signaled(signal) => (1, signal),
                    Termination::Unknown => (2, 0),
                };
                bytes.push(kind);
                bytes.extend_from_slice(&u32::to_be_bytes(value));
                bytes.push(match cause {
                    EndCause::Exited => 0,
                    EndCause::Terminated => 1,
                });
                bytes.extend_from_slice(&ended_ms.to_be_bytes());
            }
            Self::SpawnFailed(ended_ms) => {
                bytes.push(3);
                bytes.extend_from_slice(&ended_ms.to_be_bytes());
            }
            Self::NeverStarted(ended_ms) => {
                bytes.push(4);
                bytes.extend_from_slice(&ended_ms.to_be_bytes());
            }
        }
        bytes
    }

    /// Decodes a record written by [`Self::to_bytes`].
    ///
    /// # Errors
    /// Returns `InvalidData` for any other bytes.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let invalid = || io::Error::from(io::ErrorKind::InvalidData);
        let body = bytes.strip_prefix(MAGIC.as_slice()).ok_or_else(invalid)?;
        let status = match body {
            [0] => Self::Awaiting,
            [1, rest @ ..] => {
                let (pid, rest) = rest.split_first_chunk::<4>().ok_or_else(invalid)?;
                Self::Running(u32::from_be_bytes(*pid), u64::from_be_bytes(exact(rest)?))
            }
            [2, kind, rest @ ..] => {
                let (value, rest) = rest.split_first_chunk::<4>().ok_or_else(invalid)?;
                let value = u32::from_be_bytes(*value);
                let termination = match (kind, value) {
                    (0, code) => Termination::Exited(code),
                    (1, signal) => Termination::Signaled(signal),
                    (2, 0) => Termination::Unknown,
                    _ => return Err(invalid()),
                };
                let (cause, rest) = rest.split_first().ok_or_else(invalid)?;
                let cause = match cause {
                    0 => EndCause::Exited,
                    1 => EndCause::Terminated,
                    _ => return Err(invalid()),
                };
                Self::Ended(termination, cause, u64::from_be_bytes(exact(rest)?))
            }
            [3, rest @ ..] => Self::SpawnFailed(u64::from_be_bytes(exact(rest)?)),
            [4, rest @ ..] => Self::NeverStarted(u64::from_be_bytes(exact(rest)?)),
            _ => return Err(invalid()),
        };
        Ok(status)
    }
}

fn exact(bytes: &[u8]) -> io::Result<[u8; 8]> {
    <[u8; 8]>::try_from(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))
}

/// Atomically replaces the status record in a Process directory.
///
/// # Errors
/// Returns an I/O error when the record cannot be made durable.
pub fn write(directory: &Path, status: SupervisorStatus) -> io::Result<()> {
    let temporary = directory.join(TEMPORARY);
    let mut file = fs::File::create(&temporary)?;
    file.write_all(&status.to_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, directory.join(FILE))?;
    #[cfg(unix)]
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}

/// Reads the status record of a Process directory, or `None` when none was written.
///
/// # Errors
/// Returns an I/O error when the record exists but cannot be read or is malformed.
pub fn read(directory: &Path) -> io::Result<Option<SupervisorStatus>> {
    match fs::read(directory.join(FILE)) {
        Ok(bytes) => SupervisorStatus::from_bytes(&bytes).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
