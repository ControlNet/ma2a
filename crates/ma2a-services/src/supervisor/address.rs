//! Where one Process supervisor listens, and authenticated connections to it.

use std::{io, time::Duration};

use interprocess::local_socket::{
    ListenerOptions, Name,
    tokio::{Listener, RecvHalf, SendHalf, Stream, prelude::*},
};
use ma2a_core::service::ServiceError;

use super::proof::{Credentials, prove_daemon};
use crate::{FrameReader, FrameWriter};

/// Bound on connecting and proving ownership.
pub const CONNECT_DEADLINE: Duration = Duration::from_secs(5);

/// One authenticated supervisor channel.
pub type Channel = (FrameReader<RecvHalf>, FrameWriter<SendHalf>);

/// A Unix socket path in an owner-private directory, or a Windows pipe name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorAddress(String);

impl SupervisorAddress {
    /// Wraps a socket path (Unix) or pipe name (Windows).
    pub const fn new(address: String) -> Self {
        Self(address)
    }

    /// Returns the address text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn name(&self) -> io::Result<Name<'_>> {
        #[cfg(unix)]
        {
            use interprocess::local_socket::{GenericFilePath, ToFsName as _};
            self.0.as_str().to_fs_name::<GenericFilePath>()
        }
        #[cfg(windows)]
        {
            use interprocess::local_socket::{GenericNamespaced, ToNsName as _};
            self.0.as_str().to_ns_name::<GenericNamespaced>()
        }
    }

    /// Binds the listener; an existing endpoint is never reclaimed.
    ///
    /// # Errors
    /// Returns an I/O error when the address is taken or cannot be bound.
    pub fn bind(&self) -> io::Result<Listener> {
        ListenerOptions::new()
            .name(self.name()?)
            .reclaim_name(false)
            .create_tokio()
    }

    /// Connects and completes the mutual ownership proof.
    ///
    /// # Errors
    /// Returns [`ServiceError::Unreachable`] when no supervisor listens at all (no endpoint,
    /// or a refused connection), [`ServiceError::Unavailable`] when one did not answer in
    /// time, and [`ServiceError::Unauthorized`] when the peer is not this Process's supervisor.
    pub async fn connect(&self, credentials: &Credentials) -> Result<Channel, ServiceError> {
        let attempt = async {
            let name = self.name().map_err(|_| ServiceError::Unavailable)?;
            let stream = Stream::connect(name).await.map_err(|error| {
                // Only a missing or refusing endpoint proves that no supervisor listens.
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) {
                    ServiceError::Unreachable
                } else {
                    ServiceError::Unavailable
                }
            })?;
            #[cfg(unix)]
            same_user(&stream)?;
            let (receive, send) = stream.split();
            let mut channel = (FrameReader::new(receive), FrameWriter::new(send));
            prove_daemon((&mut channel.0, &mut channel.1), credentials).await?;
            Ok(channel)
        };
        tokio::time::timeout(CONNECT_DEADLINE, attempt)
            .await
            .map_err(|_| ServiceError::Unavailable)?
    }
}

/// Refuses a peer running as another user.
///
/// Windows has no peer credentials here and relies on the ownership proof alone.
///
/// # Errors
/// Returns [`ServiceError::Unauthorized`] for a peer with another effective user.
#[cfg(unix)]
pub(super) fn same_user(stream: &Stream) -> Result<(), ServiceError> {
    let credentials = stream
        .peer_creds()
        .map_err(|_| ServiceError::Unauthorized)?;
    if credentials.euid() == Some(rustix::process::geteuid().as_raw()) {
        Ok(())
    } else {
        Err(ServiceError::Unauthorized)
    }
}
