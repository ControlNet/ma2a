//! Shared authorization lease ordering `go` dispatch against Store authorization changes.

use std::{
    sync::{Arc, Condvar, Mutex, PoisonError},
    time::Instant,
};

use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GoDispatchState {
    Waiting,
    Done,
    Expired,
}

#[derive(Debug)]
struct GoDispatchLeaseInner {
    state: Mutex<GoDispatchState>,
    done: Condvar,
    expired: CancellationToken,
}

/// Shared lease between `StoreBackend::authorize_process_go` and `ProcessManager`.
///
/// Once the Store hold expires and moves from `Waiting` to `Expired`, the dispatcher cannot
/// finish writing `go` and must abort the supervisor instead.
#[derive(Clone, Debug)]
pub(crate) struct GoDispatchLease(Arc<GoDispatchLeaseInner>);

impl GoDispatchLease {
    /// Creates a new lease in the `Waiting` state.
    pub(crate) fn new() -> Self {
        Self(Arc::new(GoDispatchLeaseInner {
            state: Mutex::new(GoDispatchState::Waiting),
            done: Condvar::new(),
            expired: CancellationToken::new(),
        }))
    }

    /// Completes the lease when the dispatcher finishes or aborts before expiration.
    pub(crate) fn finish(&self) {
        let mut guard = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if *guard == GoDispatchState::Waiting {
            *guard = GoDispatchState::Done;
            drop(guard);
            self.0.done.notify_one();
        }
    }

    /// Resolves once the Store hold has expired.
    pub(crate) async fn expired(&self) {
        self.0.expired.cancelled().await;
    }

    /// Runs `step` under the lease lock while still `Waiting`; returns `None` if already
    /// `Expired` or `Done`. Marks the lease `Done` and wakes the Store when `step` reports `true`.
    pub(crate) fn try_while_waiting<R>(&self, step: impl FnOnce() -> (R, bool)) -> Option<R> {
        let mut guard = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if *guard != GoDispatchState::Waiting {
            return None;
        }
        let (result, done) = step();
        if done {
            *guard = GoDispatchState::Done;
            drop(guard);
            self.0.done.notify_one();
        } else {
            drop(guard);
        }
        Some(result)
    }

    /// Holds the Store thread until `finish` / `try_while_waiting` completes or `deadline` passes.
    pub(crate) fn wait_until(&self, deadline: Instant) {
        let mut guard = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        while *guard == GoDispatchState::Waiting {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                *guard = GoDispatchState::Expired;
                drop(guard);
                self.0.expired.cancel();
                break;
            }
            let (next, waited) = self
                .0
                .done
                .wait_timeout(guard, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            guard = next;
            if waited.timed_out() && *guard == GoDispatchState::Waiting {
                *guard = GoDispatchState::Expired;
                drop(guard);
                self.0.expired.cancel();
                break;
            }
        }
    }
}
