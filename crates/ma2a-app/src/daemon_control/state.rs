//! What is actually there, established before anything acts on it.

use std::fs::TryLockError;

use ma2a_runtime::{
    api,
    ipc::{DaemonReport, IpcError, IpcPaths, LIFECYCLE_DEADLINE, LocalApiClient},
};

use crate::AppError;

use super::open_lock;

/// The state of one state directory's daemon, as far as it can be proven.
///
/// A boolean cannot express the cases that matter. "No daemon exists" and "a
/// daemon exists and has stopped answering" call for opposite actions — launch
/// one, and never launch one — and collapsing them is exactly what lets a second
/// Runtime appear beside a wedged first. Every variant here is something a
/// caller can act on without guessing, and the two that cannot be acted on
/// safely say so instead of being rounded down to absence.
#[derive(Debug)]
pub(crate) enum DaemonState {
    /// The daemon lock is free, so no process owns this state directory.
    Absent,
    /// A daemon owns the lock and answered. `None` means it speaks the current
    /// local API but predates the lifecycle plane, so it cannot identify itself.
    Ready(Option<Box<DaemonReport>>),
    /// A daemon owns the lock and speaks a different local API version.
    Incompatible,
    /// A daemon owns the lock and did not answer a bounded lifecycle call.
    Unresponsive(String),
    /// Ownership and what answers the endpoint disagree.
    Conflicted(String),
}

impl DaemonState {
    /// Turns a state nothing may safely act on into the refusal it deserves.
    ///
    /// Both of these fail closed. `Unresponsive` must never become "start a new
    /// one" and `Conflicted` must never become "repair it", so each carries the
    /// evidence and the one manual step that resolves it.
    pub(crate) fn into_refusal(self) -> AppError {
        match self {
            Self::Unresponsive(reason) => AppError::Daemon(format!(
                "a daemon still owns this state directory but is not answering ({reason}); \
                 it was not replaced. Stop that process, then run `ma2a start`"
            )),
            Self::Conflicted(reason) => AppError::Daemon(format!(
                "daemon ownership of this state directory is ambiguous ({reason}); \
                 nothing was changed. Resolve it before starting or stopping a daemon"
            )),
            Self::Absent => AppError::DaemonStopped,
            Self::Incompatible => AppError::Ipc(IpcError::VersionMismatch),
            Self::Ready(_) => AppError::Daemon("the daemon is running".to_owned()),
        }
    }
}

/// Establishes what owns a state directory and what, if anything, answers it.
///
/// The daemon lock is the only ownership authority here. A transport failure is
/// never read as proof that the lock owner has gone: while the lock is held,
/// every way of failing to get an answer means `Unresponsive`.
pub(crate) async fn classify(paths: &IpcPaths) -> Result<DaemonState, AppError> {
    if !paths.runtime_dir().is_dir() {
        // Nothing has ever prepared this directory, so there is nothing to own.
        return Ok(DaemonState::Absent);
    }
    let lock = open_lock(&paths.lock_path())?;
    match lock.try_lock() {
        Ok(()) => {
            // Acquiring it only proves nobody else holds it. Release it at once:
            // classifying must never quietly become owning.
            drop(lock);
            Ok(match responder(paths).await {
                Responder::Silent(_) => DaemonState::Absent,
                answered => DaemonState::Conflicted(format!(
                    "the daemon lock is free but {} still answers the endpoint",
                    answered.describe()
                )),
            })
        }
        Err(TryLockError::WouldBlock) => Ok(owned_state(paths, responder(paths).await)),
        Err(TryLockError::Error(error)) => Err(error.into()),
    }
}

/// Reports whether anything is currently serving this endpoint.
///
/// A daemon that already holds the singleton lock uses this to refuse to bind
/// over a responder it cannot account for, rather than removing an endpoint that
/// something is still using.
pub(crate) async fn endpoint_answers(paths: &IpcPaths) -> bool {
    !matches!(responder(paths).await, Responder::Silent(_))
}

enum Responder {
    Lifecycle(Box<DaemonReport>),
    LocalApi,
    Incompatible,
    Silent(String),
}

impl Responder {
    const fn describe(&self) -> &'static str {
        match self {
            Self::Lifecycle(_) => "a daemon",
            Self::LocalApi => "a daemon of this API version",
            Self::Incompatible => "a daemon of another API version",
            Self::Silent(_) => "nothing",
        }
    }
}

async fn responder(paths: &IpcPaths) -> Responder {
    let client = LocalApiClient::new(paths.clone());
    match client.ping().await {
        Ok(report) if report.api_version() == u64::from(api::LOCAL_API_VERSION) => {
            Responder::Lifecycle(Box::new(report))
        }
        Ok(_) => Responder::Incompatible,
        // Answering, but not with a lifecycle record. That is a fact about the
        // responder, not about whether a daemon is there, so ask the question the
        // older contract can answer.
        Err(IpcError::NoLifecyclePlane) => legacy_responder(&client).await,
        Err(error) => Responder::Silent(error.to_string()),
    }
}

async fn legacy_responder(client: &LocalApiClient) -> Responder {
    match tokio::time::timeout(LIFECYCLE_DEADLINE, client.probe()).await {
        Ok(Ok(())) => Responder::LocalApi,
        Ok(Err(IpcError::VersionMismatch)) => Responder::Incompatible,
        Ok(Err(error)) => Responder::Silent(error.to_string()),
        Err(_elapsed) => Responder::Silent("it did not complete a handshake in time".to_owned()),
    }
}

fn owned_state(paths: &IpcPaths, responder: Responder) -> DaemonState {
    match responder {
        Responder::Lifecycle(report) => recorded_disagreement(paths, &report)
            .map_or_else(|| DaemonState::Ready(Some(report)), DaemonState::Conflicted),
        Responder::LocalApi => DaemonState::Ready(None),
        Responder::Incompatible => DaemonState::Incompatible,
        Responder::Silent(reason) => DaemonState::Unresponsive(reason),
    }
}

/// Reports a daemon record that does not describe the daemon actually answering.
///
/// Each daemon writes this record after taking the lock and before serving, so
/// with one owner the two always agree. A disagreement means more than one thing
/// has been acting as the owner, which is not something to repair by guessing.
fn recorded_disagreement(paths: &IpcPaths, answering: &DaemonReport) -> Option<String> {
    let recorded = DaemonReport::parse(&std::fs::read(paths.daemon_record_path()).ok()?)?;
    (recorded.launch_nonce() != answering.launch_nonce()
        || recorded.incarnation() != answering.incarnation())
    .then(|| {
        format!(
            "the recorded daemon (process {}) is not the daemon answering (process {})",
            recorded.incarnation().pid(),
            answering.incarnation().pid()
        )
    })
}
