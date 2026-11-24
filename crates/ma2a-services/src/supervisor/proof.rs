//! Mutual ownership proof between a daemon and one Process supervisor.
//!
//! Each side sends a fresh nonce and proves knowledge of the Process secret with keyed BLAKE3
//! over a domain-separated transcript. A socket answered by anything else — an unrelated
//! program, a reused process ID, a stale path — can never pass, so a supervisor is adopted
//! only by this proof and never by a process identifier.

use core::fmt;

use ma2a_core::service::{FrameKind, ProcessId, ServiceError};
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{FrameReader, FrameWriter};

const DOMAIN: &[u8] = b"ma2a process supervisor proof v1";

/// What both sides of one Process's supervisor channel know.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Credentials {
    id: ProcessId,
    secret: [u8; 32],
}

impl Credentials {
    /// Pairs a Process with its ownership secret.
    pub const fn new(id: ProcessId, secret: [u8; 32]) -> Self {
        Self { id, secret }
    }

    /// Returns the Process these credentials belong to.
    pub const fn id(&self) -> ProcessId {
        self.id
    }

    /// Returns the ownership secret.
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    fn mac(&self, role: &[u8], (daemon, supervisor): (&[u8; 32], &[u8; 32])) -> blake3::Hash {
        let mut hasher = blake3::Hasher::new_keyed(&self.secret);
        hasher
            .update(DOMAIN)
            .update(role)
            .update(self.id.as_bytes())
            .update(daemon)
            .update(supervisor);
        hasher.finalize()
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("id", &self.id)
            .field("secret", &"<redacted>")
            .finish()
    }
}

fn nonce() -> Result<[u8; 32], ServiceError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| ServiceError::Internal)?;
    Ok(bytes)
}

async fn control<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<Vec<u8>, ServiceError> {
    let frame = reader.expect().await?;
    if frame.kind() == FrameKind::Control {
        Ok(frame.into_payload())
    } else {
        Err(ServiceError::Unauthorized)
    }
}

fn hash(bytes: &[u8]) -> Result<blake3::Hash, ServiceError> {
    <[u8; 32]>::try_from(bytes)
        .map(blake3::Hash::from_bytes)
        .map_err(|_| ServiceError::Unauthorized)
}

/// Proves the daemon side and verifies the supervisor.
///
/// # Errors
/// Returns [`ServiceError::Unauthorized`] when the peer cannot prove the secret.
pub async fn prove_daemon<R, W>(
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    credentials: &Credentials,
) -> Result<(), ServiceError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let ours = nonce()?;
    writer.frame(FrameKind::Control, &ours).await?;
    let answer = control(reader).await?;
    let (theirs, proof) = answer
        .split_first_chunk::<32>()
        .ok_or(ServiceError::Unauthorized)?;
    // `blake3::Hash` equality is constant-time.
    if hash(proof)? != credentials.mac(b"supervisor", (&ours, theirs)) {
        return Err(ServiceError::Unauthorized);
    }
    let reply = credentials.mac(b"daemon", (&ours, theirs));
    writer.frame(FrameKind::Control, reply.as_bytes()).await
}

/// Reads the daemon's opening challenge, the first frame a supervisor connection carries.
///
/// # Errors
/// Returns [`ServiceError::Unauthorized`] for anything but a 32-byte Control frame.
pub async fn read_challenge<R: AsyncRead + Unpin>(
    reader: &mut FrameReader<R>,
) -> Result<[u8; 32], ServiceError> {
    <[u8; 32]>::try_from(control(reader).await?.as_slice()).map_err(|_| ServiceError::Unauthorized)
}

/// Proves the supervisor side to the daemon whose challenge `theirs` was read, and verifies it.
///
/// # Errors
/// Returns [`ServiceError::Unauthorized`] when the peer cannot prove the secret.
pub async fn prove_supervisor<R, W>(
    theirs: [u8; 32],
    (reader, writer): (&mut FrameReader<R>, &mut FrameWriter<W>),
    credentials: &Credentials,
) -> Result<(), ServiceError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let ours = nonce()?;
    let mut answer = ours.to_vec();
    answer.extend_from_slice(credentials.mac(b"supervisor", (&theirs, &ours)).as_bytes());
    writer.frame(FrameKind::Control, &answer).await?;
    if hash(&control(reader).await?)? == credentials.mac(b"daemon", (&theirs, &ours)) {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}
