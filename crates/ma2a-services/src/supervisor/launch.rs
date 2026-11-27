//! The launch record a daemon hands a new supervisor, and detached supervisor startup.
//!
//! The record — including the ownership secret — travels only over the supervisor's stdin
//! pipe, never in its argv or environment, where other local users could read it.

use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use ma2a_core::service::{ProcessId, ProcessStart, ServiceError};
use tokio::io::AsyncWriteExt as _;

use super::{
    address::{Channel, SupervisorAddress},
    proof::Credentials,
};

const MAGIC: &[u8; 7] = b"MA2APL1";
/// Bound on a new supervisor becoming reachable.
pub const READY_DEADLINE: Duration = Duration::from_secs(10);

/// Everything a supervisor needs to own one Process.
#[expect(
    clippy::exhaustive_structs,
    reason = "the daemon names every field so none can be forgotten"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// The Process and its ownership secret.
    pub credentials: Credentials,
    /// The Process directory: status record and output.
    pub directory: PathBuf,
    /// Where the supervisor listens.
    pub address: SupervisorAddress,
    /// The start it runs.
    pub start: ProcessStart,
}

fn push(bytes: &mut Vec<u8>, field: &[u8]) -> io::Result<()> {
    let length =
        u32::try_from(field.len()).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(field);
    Ok(())
}

fn pull<'a>(bytes: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    let invalid = || io::Error::from(io::ErrorKind::InvalidData);
    let (length, rest) = bytes.split_first_chunk::<4>().ok_or_else(invalid)?;
    let length = usize::try_from(u32::from_be_bytes(*length)).map_err(|_| invalid())?;
    let field = rest.get(..length).ok_or_else(invalid)?;
    *bytes = rest.get(length..).ok_or_else(invalid)?;
    Ok(field)
}

impl Launch {
    /// Encodes the record.
    ///
    /// # Errors
    /// Returns `InvalidInput` for a directory that is not valid Unicode.
    pub fn to_bytes(&self) -> io::Result<Vec<u8>> {
        let directory = self
            .directory
            .to_str()
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let start = self
            .start
            .to_bytes()
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(self.credentials.id().as_bytes());
        bytes.extend_from_slice(self.credentials.secret());
        push(&mut bytes, directory.as_bytes())?;
        push(&mut bytes, self.address.as_str().as_bytes())?;
        push(&mut bytes, &start)?;
        Ok(bytes)
    }

    /// Decodes a record written by [`Self::to_bytes`].
    ///
    /// # Errors
    /// Returns `InvalidData` for any other bytes.
    pub fn from_bytes(bytes: &[u8]) -> io::Result<Self> {
        let invalid = || io::Error::from(io::ErrorKind::InvalidData);
        let rest = bytes.strip_prefix(MAGIC.as_slice()).ok_or_else(invalid)?;
        let (id, rest) = rest.split_first_chunk::<16>().ok_or_else(invalid)?;
        let (secret, mut rest) = rest.split_first_chunk::<32>().ok_or_else(invalid)?;
        let text = |field: &[u8]| String::from_utf8(field.to_vec()).map_err(|_| invalid());
        let directory = PathBuf::from(text(pull(&mut rest)?)?);
        let address = SupervisorAddress::new(text(pull(&mut rest)?)?);
        let start = ProcessStart::from_bytes(pull(&mut rest)?).map_err(|_| invalid())?;
        if !rest.is_empty() || !directory.is_absolute() {
            return Err(invalid());
        }
        Ok(Self {
            credentials: Credentials::new(ProcessId::from_bytes(*id), *secret),
            directory,
            address,
            start,
        })
    }
}

/// Owns a spawned supervisor until it proves ownership; any pre-proof failure or
/// cancellation terminates and reaps it instead of leaving an unproven process behind.
struct UnprovenChild(Option<tokio::process::Child>);

impl UnprovenChild {
    async fn terminate(mut self) {
        if let Some(mut child) = self.0.take() {
            let _killed = child.kill().await;
        }
    }

    fn into_child(mut self) -> Option<tokio::process::Child> {
        self.0.take()
    }
}

impl Drop for UnprovenChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _killed = child.start_kill();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _reaped = child.wait().await;
                });
            }
        }
    }
}

/// Starts a detached supervisor and waits until it proves ownership.
///
/// On Unix the supervisor makes itself a session leader; on Windows it is created detached
/// in its own process group. Either way it outlives this process. The returned child is
/// only for reaping; dropping it never stops the supervisor.
///
/// # Errors
/// Returns [`ServiceError::Unavailable`] when the supervisor cannot be started or never
/// becomes reachable.
pub async fn spawn(
    (program, args): (&Path, &[OsString]),
    launch: &Launch,
) -> Result<(tokio::process::Child, Channel), ServiceError> {
    let record = launch.to_bytes().map_err(|_| ServiceError::Internal)?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false);
    #[cfg(windows)]
    {
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let deadline = tokio::time::Instant::now() + READY_DEADLINE;
    let mut child = UnprovenChild(Some(
        command.spawn().map_err(|_| ServiceError::Unavailable)?,
    ));
    let Some(mut stdin) = child.0.as_mut().and_then(|child| child.stdin.take()) else {
        child.terminate().await;
        return Err(ServiceError::Internal);
    };
    let written = tokio::time::timeout_at(deadline, stdin.write_all(&record)).await;
    drop(stdin);
    let channel = match written {
        Ok(Ok(())) => ready_until(&launch.address, &launch.credentials, deadline).await,
        Ok(Err(_)) | Err(_) => Err(ServiceError::Unavailable),
    };
    match channel {
        Ok(channel) => child
            .into_child()
            .map(|child| (child, channel))
            .ok_or(ServiceError::Internal),
        Err(error) => {
            child.terminate().await;
            Err(error)
        }
    }
}

/// Connects to a supervisor that is starting, retrying until [`READY_DEADLINE`].
///
/// # Errors
/// Returns [`ServiceError::Unavailable`] when it never becomes reachable, and
/// [`ServiceError::Unauthorized`] when something else answers.
pub async fn ready(
    address: &SupervisorAddress,
    credentials: &Credentials,
) -> Result<Channel, ServiceError> {
    ready_until(
        address,
        credentials,
        tokio::time::Instant::now() + READY_DEADLINE,
    )
    .await
}

async fn ready_until(
    address: &SupervisorAddress,
    credentials: &Credentials,
    deadline: tokio::time::Instant,
) -> Result<Channel, ServiceError> {
    loop {
        match address.connect(credentials).await {
            Err(ServiceError::Unavailable | ServiceError::Unreachable)
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            result => return result,
        }
    }
}
