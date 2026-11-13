//! Hosted services: third-party programs that serve calls through this Runtime.
//!
//! `SQLite` holds every registration and whether it is started. A service process attaches
//! one link over the local API; links, pending calls, and counters live only in memory, so
//! after any restart a started service is unavailable until its process attaches again. The
//! Runtime keeps identity, authorization, and framing: a service only ever learns calls the
//! Runtime has authorized: the caller's Endpoint, `RequestId`, version, operation, and body.

mod call;
mod describe;
mod link;
mod operator;

use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use ma2a_core::{
    EndpointId, RequestId,
    service::{MAX_SERVICE_CALLS, OperationName, ServiceError, ServiceName},
};
use ma2a_store::{HostedServiceRecord, Repository};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

pub(crate) use call::serve;
pub(crate) use link::LinkHandle;

use crate::{
    RuntimeClock,
    api::{HostedServiceState, HostedServiceView},
    store::StoreClient,
};

/// Most services one Runtime registers.
pub(crate) const MAX_HOSTED_SERVICES: usize = 64;
/// Most notices one link buffers: one per running call, and the stop.
const NOTICE_BUFFER: usize = MAX_SERVICE_CALLS as usize + 1;

/// One accepted call's connection.
pub(crate) trait CallIo: AsyncRead + AsyncWrite + Send + Unpin {}

impl<T: AsyncRead + AsyncWrite + Send + Unpin> CallIo for T {}

/// The connection a service accepted one call on, and the exact answer it is still owed.
///
/// The answer is written by the call, not by the accepting connection, so a call is either
/// withdrawn before its service learned anything or delivered: never both.
pub(crate) struct Accepted {
    pub(crate) stream: Box<dyn CallIo>,
    pub(crate) answer: Vec<u8>,
}

/// What a service learns about one call when it accepts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallDetails {
    pub(crate) caller: EndpointId,
    pub(crate) request_id: RequestId,
    pub(crate) version: u16,
    pub(crate) operation: OperationName,
    pub(crate) body: Vec<u8>,
}

/// One message the Runtime sends down a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Notice {
    /// A call waits for the service to accept it.
    Call([u8; 16]),
    /// The operator stopped the service; the link ends after this.
    Stop,
}

struct Link {
    id: u64,
    ready: bool,
    notices: mpsc::Sender<Notice>,
}

#[derive(Default)]
struct Live {
    link: Option<Link>,
    /// Running calls; each call's slot holds it, so a call that outlives this registration
    /// never counts against a later one of the same name.
    running: Arc<AtomicUsize>,
    calls: BTreeMap<u16, u64>,
    /// Cancelled when the operator stops the service; replaced when it starts again.
    stop: CancellationToken,
}

struct Pending {
    service: ServiceName,
    details: CallDetails,
    deliver: oneshot::Sender<Accepted>,
}

#[derive(Default)]
struct State {
    services: HashMap<ServiceName, Live>,
    pending: HashMap<[u8; 16], Pending>,
    links: u64,
}

struct Inner {
    local: EndpointId,
    store: StoreClient,
    clock: Arc<dyn RuntimeClock>,
    /// Serializes register, start, stop, and attach, so a link never outlives a stop.
    lifecycle: tokio::sync::Mutex<()>,
    state: Mutex<State>,
}

/// Every service this Runtime hosts; cheap to clone.
#[derive(Clone)]
pub(crate) struct HostedServices(Arc<Inner>);

impl fmt::Debug for HostedServices {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedServices")
            .field("local", &self.0.local)
            .finish_non_exhaustive()
    }
}

impl HostedServices {
    /// Creates the manager; nothing is linked until a service process attaches.
    pub(crate) fn new(
        (local, store, clock): (EndpointId, StoreClient, Arc<dyn RuntimeClock>),
    ) -> Self {
        Self(Arc::new(Inner {
            local,
            store,
            clock,
            lifecycle: tokio::sync::Mutex::new(()),
            state: Mutex::new(State::default()),
        }))
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now_ms(&self) -> u64 {
        self.0
            .clock
            .now_ms()
            .ok()
            .and_then(|now| u64::try_from(now).ok())
            .unwrap_or(0)
    }

    /// Runs one repository job on the store thread, mapping every failure to unavailable.
    async fn store<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Repository) -> Result<T, ma2a_store::StoreError> + Send + 'static,
    ) -> Result<T, ServiceError> {
        self.0
            .store
            .processes(job)
            .await?
            .map_err(|_| ServiceError::Unavailable)
    }

    async fn registration(
        &self,
        name: &ServiceName,
    ) -> Result<Option<HostedServiceRecord>, ServiceError> {
        let name = name.clone();
        self.store(move |repository| repository.hosted_service(&name))
            .await
    }

    /// Counts one call of `version`, as an admitted call does.
    #[cfg(test)]
    pub(crate) fn count_call(&self, name: &ServiceName, version: u16) {
        *self
            .state()
            .services
            .entry(name.clone())
            .or_default()
            .calls
            .entry(version)
            .or_default() += 1;
    }

    /// Returns one registration with what runs now.
    fn observe(&self, record: HostedServiceRecord) -> HostedServiceView {
        let state = self.state();
        let live = state.services.get(&record.name);
        let ready = live
            .and_then(|live| live.link.as_ref())
            .is_some_and(|link| link.ready);
        let running = live.map_or(0, |live| live.running.load(Ordering::Relaxed));
        let calls = live.map_or_else(Vec::new, |live| {
            live.calls
                .iter()
                .map(|(version, count)| (*version, *count))
                .collect()
        });
        let current = match (record.enabled, ready) {
            (false, _) => HostedServiceState::Stopped,
            (true, false) => HostedServiceState::Unavailable,
            (true, true) => HostedServiceState::Ready,
        };
        drop(state);
        HostedServiceView::new(record, (current, running, calls))
    }
}

/// Aborts this daemon at one named point (debug builds only).
fn fault(point: &str) {
    #[cfg(debug_assertions)]
    if std::env::var_os("MA2A_TEST_SERVICE_FAULT").is_some_and(|value| value == point) {
        ma2a_services::supervisor::crash();
    }
    #[cfg(not(debug_assertions))]
    let _ = point;
}
