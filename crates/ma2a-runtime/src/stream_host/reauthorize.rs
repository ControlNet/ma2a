//! Running inbound operations stay authorized for as long as they run.
//!
//! Authorization is decided at open, but a Shell, Proxy, Exec, or Transfer can outlive that
//! decision by hours. Whenever durable state changes, every running inbound operation is
//! authorized again for the exact access it opened with; one that no longer passes —
//! because its peer was revoked, its grant removed, its service unexposed, or this Endpoint
//! left the Space — is cancelled.

use std::{
    collections::BTreeMap,
    future::Future,
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
        if seen == Some(revision) {
            continue;
        }
        let watched = context.authorizations.snapshot();
        let store = &context.store;
        let decided = recheck(watched, |peer, access| {
            store.authorize_service((context.local, peer), access)
        })
        .await;
        // A revision counts as checked only once every operation got a definite answer;
        // one the Store could not answer is asked again on the next tick.
        if decided {
            seen = Some(revision);
        }
    }
}

/// Authorizes each of `checks` again, cancelling every one that no longer passes. Returns
/// whether each got a definite answer: `Unavailable` decides nothing, and neither does a
/// grant for an operation that has not bound its root yet, since the root it is about to use
/// may come from an older state than this grant.
async fn recheck<Authorize, Decision>(checks: Vec<Check>, authorize: Authorize) -> bool
where
    Authorize: Fn(EndpointId, ServiceAccess) -> Decision,
    Decision: Future<Output = Result<crate::store::ServiceGrant, ServiceError>>,
{
    let mut decided = true;
    for (peer, access, root, stop) in checks {
        match (authorize(peer, access).await, root) {
            (Ok(grant), Bound::Root(bound)) if bound == grant.root => {}
            // Unavailable: either the Store failed this read or it is stopping with the
            // Runtime, when shutdown ends the operation anyway.
            (Ok(_), Bound::Pending) | (Err(ServiceError::Unavailable), _) => decided = false,
            (Ok(_) | Err(_), _) => stop.cancel(),
        }
    }
    decided
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use ma2a_core::service::{ServiceAccess, ServiceError};
    use tokio_util::sync::CancellationToken;

    use std::path::PathBuf;

    use super::{Bound, recheck};
    use crate::store::ServiceGrant;

    #[tokio::test]
    async fn a_check_the_store_could_not_answer_is_decided_on_a_later_tick()
    -> Result<(), ma2a_core::ProtocolError> {
        // Given: a revoked operation, and a Store whose first answer is unavailable.
        let key = ma2a_core::SpaceAuthoritySecret::from_bytes([7; 32]).public_key();
        let peer = ma2a_core::EndpointId::try_from(key.as_bytes().as_slice())?;
        let stop = CancellationToken::new();
        let check = || vec![(peer, ServiceAccess::Exec, Bound::Root(None), stop.clone())];
        let asked = AtomicUsize::new(0);
        let authorize = |_, _| {
            let first = asked.fetch_add(1, Ordering::Relaxed) == 0;
            async move {
                Err(if first {
                    ServiceError::Unavailable
                } else {
                    ServiceError::Unauthorized
                })
            }
        };

        // When / Then: the first pass decides nothing and keeps the operation; the next
        // cancels it.
        assert!(!recheck(check(), &authorize).await);
        assert!(!stop.is_cancelled());
        assert!(recheck(check(), &authorize).await);
        assert!(stop.is_cancelled());
        Ok(())
    }

    #[tokio::test]
    async fn an_operation_still_binding_its_root_is_checked_again_once_bound()
    -> Result<(), ma2a_core::ProtocolError> {
        // Given: an operation authorized for `old`, not yet bound, while the name now resolves
        // to `new`.
        let key = ma2a_core::SpaceAuthoritySecret::from_bytes([7; 32]).public_key();
        let peer = ma2a_core::EndpointId::try_from(key.as_bytes().as_slice())?;
        let stop = CancellationToken::new();
        let access = ServiceAccess::Exec;
        let authorize = |_, _| async {
            Ok(ServiceGrant {
                root: Some(PathBuf::from("new")),
            })
        };

        // When / Then: the pending pass decides nothing, so the revision is checked again;
        // once bound to `old`, the operation is cancelled.
        let pending = vec![(peer, access.clone(), Bound::Pending, stop.clone())];
        assert!(!recheck(pending, &authorize).await);
        assert!(!stop.is_cancelled());
        let bound = Bound::Root(Some(PathBuf::from("old")));
        assert!(recheck(vec![(peer, access, bound, stop.clone())], &authorize).await);
        assert!(stop.is_cancelled());
        Ok(())
    }
}
