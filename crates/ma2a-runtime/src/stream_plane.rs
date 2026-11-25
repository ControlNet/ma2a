//! Local stream plane: remote service operations a same-user client runs through this Runtime.
//!
//! The client never owns Endpoint identity. It hands the Runtime one Services v1 stream, and
//! the Runtime dials the target as itself and relays frames verbatim.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};

use ma2a_core::{EndpointId, RequestId, ServiceKind, service::OpenBody};

pub(crate) mod relay;

/// How long one frame may wait for room in either direction of `body`'s relay.
///
/// Proxy operations honor the request's `idle_timeout_ms` plus [`relay::CANCEL_GRACE`] so the
/// target's blocked-write idle timeout and the caller's local-write idle timeout can settle
/// cleanly before the stream plane gives up.
pub(crate) fn stall_for(body: &OpenBody) -> Duration {
    match body {
        OpenBody::Proxy(proxy) => relay::FORWARD_STALL.max(
            Duration::from_millis(proxy.idle_timeout_ms()).saturating_add(relay::CANCEL_GRACE),
        ),
        _ => relay::FORWARD_STALL,
    }
}

/// Most concurrent stream-plane connections, separate from the business connection pool.
pub(crate) const STREAM_CONNECTION_LIMIT: usize = 64;

/// Request IDs of stream-plane operations running in this process.
///
/// A durable reservation without a running operation was left by an earlier process, so its
/// effect is unknown; one with a running operation is merely a concurrent duplicate.
#[derive(Clone, Debug, Default)]
pub(crate) struct InFlight(Arc<Mutex<HashSet<RequestId>>>);

impl InFlight {
    /// Marks one request running, or returns `None` when it already is.
    pub(crate) fn begin(&self, request_id: RequestId) -> Option<InFlightGuard> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(request_id)
            .then(|| InFlightGuard {
                set: self.clone(),
                request_id,
            })
    }

    /// Returns whether one request is running in this process.
    pub(crate) fn contains(&self, request_id: RequestId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&request_id)
    }
}

/// Keeps one request marked running until dropped.
#[derive(Debug)]
pub(crate) struct InFlightGuard {
    set: InFlight,
    request_id: RequestId,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.set
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.request_id);
    }
}

/// Fingerprints one side-effecting stream request: its family, exact target, and exact
/// canonical `open` payload. It is domain-separated from local API command fingerprints,
/// which share the replay table, so the two can never be confused.
pub(crate) fn fingerprint(kind: ServiceKind, target: EndpointId, open: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("ma2a stream-plane open v1");
    hasher.update(&[kind.code()]);
    hasher.update(target.as_bytes());
    hasher.update(open);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use ma2a_core::{EndpointId, RequestId, ServiceKind};

    use super::{InFlight, fingerprint};

    #[test]
    fn a_running_request_is_exclusive_until_its_guard_drops() {
        let running = InFlight::default();
        let id = RequestId::try_from([9_u8; 16].as_slice()).expect("request id");
        let guard = running.begin(id).expect("first begin");
        assert!(running.begin(id).is_none());
        assert!(running.contains(id));
        drop(guard);
        assert!(!running.contains(id));
        assert!(running.begin(id).is_some());
    }

    #[test]
    fn the_fingerprint_binds_family_target_and_exact_open() {
        let one = EndpointId::from(iroh::SecretKey::generate().public());
        let two = EndpointId::from(iroh::SecretKey::generate().public());
        let base = fingerprint(ServiceKind::Exec, one, b"open");
        assert_eq!(base, fingerprint(ServiceKind::Exec, one, b"open"));
        assert_ne!(base, fingerprint(ServiceKind::Shell, one, b"open"));
        assert_ne!(base, fingerprint(ServiceKind::Exec, two, b"open"));
        assert_ne!(base, fingerprint(ServiceKind::Exec, one, b"opem"));
    }
}
