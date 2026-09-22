//! The one place a Web request reaches the private Runtime transport.

use std::time::Duration;

use crate::{api::Command, ipc::LocalApiClient};

/// Bounds one Web-originated Runtime call so an HTTP request always answers.
///
/// The private transport imposes no reply deadline of its own, because a local
/// Runtime that stops closes its socket and the read then ends exactly. An HTTP
/// handler cannot inherit that patience: a browser request must be answered or
/// refused, and a connection held open forever is itself a failure. This bound
/// therefore belongs to HTTP rather than to the transport. It is deliberately
/// far longer than any Runtime operation and is never set per operation, so it
/// never becomes a guess about how long legitimate work takes.
const WEB_CALL_DEADLINE: Duration = Duration::from_secs(120);

/// Sends one command and reports failure without distinguishing its cause.
///
/// # Errors
/// Returns unit when the Runtime refused, stopped, or exceeded the HTTP bound.
pub(super) async fn call(runtime: &LocalApiClient, command: &Command) -> Result<Vec<u8>, ()> {
    tokio::time::timeout(WEB_CALL_DEADLINE, runtime.call(command))
        .await
        .map_err(|_timeout| ())?
        .map_err(|_refused| ())
}
