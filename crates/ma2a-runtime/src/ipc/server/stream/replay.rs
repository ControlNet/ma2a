//! Durable replay for side-effecting stream-plane operations.
//!
//! The replay lock is held only while a decision is made, never while streaming.

use ma2a_core::{
    RequestId,
    service::{FrameKind, ServiceError, failed_outcome},
};
use ma2a_store::{MutationReplayRecord, MutationReplayRequest, MutationReplayState};

use crate::{
    RuntimeHandle,
    stream_plane::{InFlightGuard, relay::encoded},
};

use super::super::ConnectionContext;

/// What to do with one side-effecting `open`.
pub(super) enum Admission {
    /// Run the operation; its terminal must settle the reservation.
    Proceed(Reservation),
    /// Answer with this exact frame and contact nobody.
    Answer(Vec<u8>),
}

/// One durably reserved request that must be released or completed.
pub(super) struct Reservation {
    handle: RuntimeHandle,
    request: MutationReplayRequest,
    revision: u64,
    _running: InFlightGuard,
}

impl Reservation {
    /// Releases a request that provably had no effect: a failed dial or a target `reject`.
    pub(super) async fn release(self) {
        let _released = self
            .handle
            .abort_mutation_replay(self.request.request_id())
            .await;
    }

    /// Retains the exact terminal frame the same request receives from now on.
    ///
    /// A terminal beyond the replay bound stays reserved; once nothing runs it, the same
    /// request is answered as unknown, which never invites a second effect.
    pub(super) async fn complete(self, terminal: Vec<u8>) {
        if let Ok(record) = MutationReplayRecord::new(self.request, self.revision, terminal) {
            let _recorded = self.handle.record_mutation_replay(record).await;
        }
    }
}

/// Returns the exact frame of a failed `outcome` whose effect is unknown.
pub(super) fn outcome_unknown() -> Vec<u8> {
    failed_outcome(ServiceError::OutcomeUnknown)
        .map_err(|_| ServiceError::Internal)
        .and_then(|payload| encoded(FrameKind::Outcome, &payload))
        .unwrap_or_default()
}

fn reject(error: ServiceError) -> Admission {
    Admission::Answer(encoded(FrameKind::Reject, &[error.code()]).unwrap_or_default())
}

/// Decides one side-effecting request under the shared replay lock.
pub(super) async fn admit(
    context: &ConnectionContext,
    (request_id, fingerprint): (RequestId, [u8; 32]),
    revision: u64,
) -> Admission {
    let _replay = context.replay.lock().await;
    match context.handle.mutation_replay(request_id).await {
        Ok(None) => {
            let Some(running) = context.in_flight.begin(request_id) else {
                return reject(ServiceError::Unavailable);
            };
            if context
                .handle
                .reserve_mutation_replay(request_id, fingerprint)
                .await
                .is_err()
            {
                return reject(ServiceError::Unavailable);
            }
            Admission::Proceed(Reservation {
                handle: context.handle.clone(),
                request: MutationReplayRequest::new(request_id, fingerprint),
                revision,
                _running: running,
            })
        }
        Ok(Some(MutationReplayState::Completed(entry))) if entry.fingerprint() == fingerprint => {
            Admission::Answer(entry.response().to_vec())
        }
        Ok(Some(MutationReplayState::Pending(found))) if found == fingerprint => {
            if context.in_flight.contains(request_id) {
                reject(ServiceError::Unavailable)
            } else {
                // Reserved by an earlier process that never settled it, or retained only as a
                // fence after its response was evicted: either way the effect is unknown.
                Admission::Answer(outcome_unknown())
            }
        }
        Ok(Some(MutationReplayState::Pending(_) | MutationReplayState::Completed(_))) => {
            reject(ServiceError::Conflict)
        }
        Err(_) | Ok(Some(_)) => reject(ServiceError::Unavailable),
    }
}
