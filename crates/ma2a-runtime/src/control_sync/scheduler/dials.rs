use std::{collections::BTreeSet, future::Future, pin::Pin};

use ma2a_core::EndpointId;
use ma2a_net::CONTROL_DIAL_CONCURRENCY;
use tokio::{task::JoinSet, time::Sleep};

use crate::{
    control_sync::{ControlApplyOutcome, ControlFailure, ControlRoundOutcome, PreparedControlPeer},
    error::RuntimeError,
};

use super::DialQueue;

#[derive(Default)]
pub(super) struct RoundProgress {
    pub(super) dialed_peers: BTreeSet<EndpointId>,
    pub(super) latest: Option<u64>,
    pub(super) changes: crate::control_sync::ControlChanges,
}

impl RoundProgress {
    pub(super) const fn record(
        &mut self,
        revision: u64,
        applied: crate::control_sync::ControlChanges,
    ) {
        self.changes.merge(applied);
        self.latest = Some(revision);
    }

    pub(super) fn finish_error(
        self,
        failure: ControlFailure,
    ) -> Result<Option<ControlRoundOutcome>, ControlFailure> {
        match failure {
            ControlFailure::Rejected(_) => {}
            ControlFailure::Runtime(error) => {
                crate::actor::Actor::absorb_background("control completion", Err(error))?;
            }
        }
        Ok(Some(ControlRoundOutcome {
            revision: self.latest,
            dialed_peers: self.dialed_peers,
            synchronized_peers: BTreeSet::new(),
            changes: self.changes,
            incomplete: true,
        }))
    }
}

pub(super) async fn await_store_apply_receipt<DialError: 'static>(
    deadline: &mut Pin<&mut Sleep>,
    dials: &mut JoinSet<DialOutcome<DialError>>,
    mut admitted: crate::store_client::control::AdmittedControlApply,
) -> Result<Result<ControlApplyOutcome, ControlFailure>, RuntimeError> {
    let applied = tokio::select! {
        biased;
        applied = &mut admitted.response => applied,
        () = deadline.as_mut() => {
            let cancelled = admitted.lease.cancel();
            dials.shutdown().await;
            if cancelled {
                return Ok(Err(ma2a_net::ControlRejection::Unavailable.into()));
            }
            admitted.response.await
        }
    };
    applied.map_err(crate::store::channel_error)
}

pub(super) async fn ensure_before_deadline(
    deadline: &mut Pin<&mut Sleep>,
) -> Result<(), ControlFailure> {
    run_before_deadline(deadline, std::future::ready(())).await
}

pub(super) async fn run_before_deadline<Output>(
    deadline: &mut Pin<&mut Sleep>,
    operation: impl Future<Output = Output>,
) -> Result<Output, ControlFailure> {
    tokio::select! {
        biased;
        () = deadline.as_mut() => Err(ma2a_net::ControlRejection::Unavailable.into()),
        output = operation => Ok(output),
    }
}

pub(super) type DialOutcome<DialError> = (
    EndpointId,
    Vec<ma2a_core::SpaceId>,
    Result<Vec<u8>, DialError>,
);

pub(super) fn spawn_control_dials<Dial, DialFuture, DialError>(
    (pending, dialed_peers): (&mut DialQueue, &mut BTreeSet<EndpointId>),
    dials: &mut JoinSet<DialOutcome<DialError>>,
    dial: Dial,
) where
    Dial: Fn(EndpointId, Vec<u8>) -> DialFuture + Clone + Send + 'static,
    DialFuture: Future<Output = Result<Vec<u8>, DialError>> + Send + 'static,
    DialError: Send + 'static,
{
    while dials.len() < CONTROL_DIAL_CONCURRENCY {
        let Some(PreparedControlPeer {
            peer,
            request,
            spaces,
        }) = pending.next()
        else {
            break;
        };
        dialed_peers.insert(peer);
        let call = dial(peer, request);
        dials.spawn(async move { (peer, spaces, call.await) });
    }
}
