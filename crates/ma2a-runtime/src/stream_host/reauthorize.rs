//! Running inbound operations stay authorized for as long as they run.
//!
//! Authorization is decided at open, but a Shell, Proxy, Exec, or Transfer can outlive that
//! decision by hours. Whenever durable state changes, every running inbound operation is
//! authorized again for the exact access it opened with; one that no longer passes —
//! because its peer was revoked, its grant removed, its service unexposed, or this Endpoint
//! left the Space — is cancelled.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use ma2a_core::{EndpointId, service::ServiceAccess, service::ServiceError};
use tokio_util::sync::CancellationToken;

use super::HostContext;

/// How often durable state is checked for a change that may withdraw authorization.
pub(crate) const REAUTHORIZE_INTERVAL: Duration = Duration::from_secs(1);

/// The root an operation resolved when it was authorized.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Bound {
    /// The operation has not finished its own authorization yet.
    Pending,
    /// The local directory of its root, or `None` for a service without one.
    Root(Option<PathBuf>),
}

impl Bound {
    /// Whether a fresh authorization still resolves the same root.
    fn admits(&self, root: Option<&PathBuf>) -> bool {
        match self {
            Self::Pending => true,
            Self::Root(bound) => bound.as_ref() == root,
        }
    }
}

#[derive(Debug)]
struct Watched {
    peer: EndpointId,
    access: ServiceAccess,
    root: Bound,
    stop: CancellationToken,
}

type Check = (EndpointId, ServiceAccess, Bound, CancellationToken);

#[derive(Debug, Default)]
struct Inner {
    next: u64,
    watched: BTreeMap<u64, Watched>,
}

/// Every running inbound operation with the access it must keep.
#[derive(Clone, Debug, Default)]
pub(crate) struct Authorizations(Arc<Mutex<Inner>>);

/// Keeps one operation watched until it is dropped.
#[derive(Debug)]
pub(crate) struct WatchGuard {
    registry: Authorizations,
    id: u64,
}

impl Authorizations {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Watches one operation from before its own authorization, so no change can fall
    /// between that decision and the first re-check.
    pub(crate) fn watch(
        &self,
        (peer, access): (EndpointId, ServiceAccess),
        stop: CancellationToken,
    ) -> WatchGuard {
        let watched = Watched {
            peer,
            access,
            root: Bound::Pending,
            stop,
        };
        let id = {
            let mut inner = self.lock();
            inner.next = inner.next.wrapping_add(1);
            let id = inner.next;
            inner.watched.insert(id, watched);
            id
        };
        WatchGuard {
            registry: self.clone(),
            id,
        }
    }

    fn snapshot(&self) -> Vec<Check> {
        self.lock()
            .watched
            .values()
            .map(|watched| {
                let access = watched.access.clone();
                (
                    watched.peer,
                    access,
                    watched.root.clone(),
                    watched.stop.clone(),
                )
            })
            .collect()
    }
}

impl WatchGuard {
    /// Records the root the operation was authorized for; a later grant must resolve the same.
    pub(crate) fn bind(&self, root: Option<PathBuf>) {
        if let Some(watched) = self.registry.lock().watched.get_mut(&self.id) {
            watched.root = Bound::Root(root);
        }
    }
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.registry.lock().watched.remove(&self.id);
    }
}

/// Re-authorizes every running inbound operation after each durable state change.
pub(crate) async fn run(context: HostContext) {
    let mut interval = tokio::time::interval(REAUTHORIZE_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut seen = None;
    loop {
        tokio::select! {
            biased;
            () = context.shutdown.cancelled() => return,
            _tick = interval.tick() => {}
        }
        // The revision is read before the snapshot: an operation registered later is then
        // authorized against a state at least this new, and any later change moves it on.
        let Ok(revision) = context.store.revision().await else {
            continue;
        };
        let watched = context.authorizations.snapshot();
        let changed = seen != Some(revision);
        seen = Some(revision);
        if !changed || watched.is_empty() {
            continue;
        }
        for (peer, access, root, stop) in watched {
            let decision = context
                .store
                .authorize_service((context.local, peer), access)
                .await;
            match decision {
                Ok(grant) if root.admits(grant.root.as_ref()) => {}
                // The store is stopping with the Runtime; shutdown ends the operation.
                Err(ServiceError::Unavailable) => {}
                Ok(_) | Err(_) => stop.cancel(),
            }
        }
    }
}
