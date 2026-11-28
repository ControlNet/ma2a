use std::{collections::BTreeSet, future::Future, pin::Pin, sync::Arc};

use ma2a_net::{
    CONTROL_DIAL_CONCURRENCY, CONTROL_ROUND_DEADLINE, ControlClient, exchange_control_with_retry,
};
use tokio::task::JoinSet;
use tokio::time::Sleep;

use crate::{
    RuntimeClock,
    control_sync::{
        ControlApplyOutcome, ControlExchangeInput, ControlFailure, ControlRoundOutcome,
        ControlRoundRequest, PreparedControlPeer, dial_queue::DialQueue,
    },
    error::{RuntimeError, RuntimeErrorKind},
    store::StoreClient,
};

pub(crate) struct ControlRoundRunner {
    store: StoreClient,
    client: ControlClient,
    clock: Arc<dyn RuntimeClock>,
}

impl ControlRoundRunner {
    pub(crate) const fn new(
        store: StoreClient,
        client: ControlClient,
        clock: Arc<dyn RuntimeClock>,
    ) -> Self {
        Self {
            store,
            client,
            clock,
        }
    }

    pub(crate) async fn run(
        self,
        request: ControlRoundRequest,
    ) -> Result<Option<ControlRoundOutcome>, ControlFailure> {
        let client = self.client.clone();
        self.run_with_dialer(request, move |peer, request| {
            let client = client.clone();
            async move { exchange_control_with_retry(&client, peer, &request).await }
        })
        .await
    }

    async fn run_with_dialer<Dial, DialFuture, DialError>(
        self,
        request: ControlRoundRequest,
        dial: Dial,
    ) -> Result<Option<ControlRoundOutcome>, ControlFailure>
    where
        Dial: Fn(ma2a_core::EndpointId, Vec<u8>) -> DialFuture + Clone + Send + 'static,
        DialFuture: Future<Output = Result<Vec<u8>, DialError>> + Send + 'static,
        DialError: Send + 'static,
    {
        let mut progress = RoundProgress::default();
        let deadline = tokio::time::sleep(CONTROL_ROUND_DEADLINE);
        tokio::pin!(deadline);
        match self
            .run_within_deadline((request, dial), (&mut progress, deadline))
            .await
        {
            Ok(outcome) => Ok(outcome),
            Err(failure) => progress.finish_error(failure),
        }
    }

    async fn run_within_deadline<Dial, DialFuture, DialError>(
        self,
        (request, dial): (ControlRoundRequest, Dial),
        (progress, mut deadline): (&mut RoundProgress, Pin<&mut Sleep>),
    ) -> Result<Option<ControlRoundOutcome>, ControlFailure>
    where
        Dial: Fn(ma2a_core::EndpointId, Vec<u8>) -> DialFuture + Clone + Send + 'static,
        DialFuture: Future<Output = Result<Vec<u8>, DialError>> + Send + 'static,
        DialError: Send + 'static,
    {
        let local_endpoint_id = request.local_endpoint_id;
        let prepared =
            run_before_deadline(&mut deadline, self.store.prepare_control_round(request)).await??;
        let mut pending = DialQueue::new(prepared);
        let had_peers = !pending.is_empty();
        let mut dials = JoinSet::new();
        let mut synchronized_peers = BTreeSet::new();
        while !pending.is_empty() || !dials.is_empty() {
            ensure_before_deadline(&mut deadline).await?;
            spawn_control_dials(
                (&mut pending, &mut progress.dialed_peers),
                &mut dials,
                dial.clone(),
            );
            let Some(joined) = run_before_deadline(&mut deadline, dials.join_next()).await? else {
                break;
            };
            let (peer, spaces, response) = joined.map_err(RuntimeError::from)?;
            let Ok(response) = response else {
                pending.finished(peer, false);
                continue;
            };
            let now_ms = self.clock.now_ms().and_then(|value| {
                u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
            })?;
            let round_deadline = deadline.deadline();
            let admitted = run_before_deadline(
                &mut deadline,
                self.store.admit_control_response(
                    (
                        ControlExchangeInput {
                            local_endpoint_id,
                            remote_endpoint_id: peer,
                            payload: response,
                            now_ms,
                        },
                        spaces,
                    ),
                    round_deadline,
                ),
            )
            .await??;
            // Cancel a still-queued apply at the deadline, or await its receipt if the Store
            // already started applying it so any committed transaction reaches Runtime state.
            let applied = await_store_apply_receipt(&mut deadline, &mut dials, admitted).await?;
            // A response this peer got wrong, such as one answering another request's Spaces,
            // fails that request only; a local failure still ends the round.
            match applied {
                Ok(ControlApplyOutcome {
                    revision,
                    changes: applied,
                }) => {
                    pending.finished(peer, true);
                    synchronized_peers.insert(peer);
                    progress.record(revision, applied);
                }
                Err(ControlFailure::Rejected(_)) => {
                    pending.finished(peer, false);
                }
                Err(failure) => return Err(failure),
            }
            ensure_before_deadline(&mut deadline).await?;
        }
        // A peer whose Spaces were split across requests is synchronized only if all of them
        // succeeded.
        synchronized_peers.retain(|peer| !pending.failed().contains(peer));
        if had_peers && progress.latest.is_none() {
            return Err(ma2a_net::ControlRejection::Unavailable.into());
        }
        let incomplete = had_peers && synchronized_peers.is_empty();
        Ok(progress.latest.map(|revision| ControlRoundOutcome {
            revision: Some(revision),
            dialed_peers: std::mem::take(&mut progress.dialed_peers),
            synchronized_peers,
            changes: std::mem::take(&mut progress.changes),
            incomplete,
        }))
    }
}

#[derive(Default)]
struct RoundProgress {
    dialed_peers: BTreeSet<ma2a_core::EndpointId>,
    latest: Option<u64>,
    changes: crate::control_sync::ControlChanges,
}

impl RoundProgress {
    const fn record(&mut self, revision: u64, applied: crate::control_sync::ControlChanges) {
        self.changes.merge(applied);
        self.latest = Some(revision);
    }

    fn finish_error(
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

async fn await_store_apply_receipt<DialError: 'static>(
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

async fn ensure_before_deadline(deadline: &mut Pin<&mut Sleep>) -> Result<(), ControlFailure> {
    run_before_deadline(deadline, std::future::ready(())).await
}

async fn run_before_deadline<Output>(
    deadline: &mut Pin<&mut Sleep>,
    operation: impl Future<Output = Output>,
) -> Result<Output, ControlFailure> {
    tokio::select! {
        biased;
        () = deadline.as_mut() => Err(ma2a_net::ControlRejection::Unavailable.into()),
        output = operation => Ok(output),
    }
}

type DialOutcome<DialError> = (
    ma2a_core::EndpointId,
    Vec<ma2a_core::SpaceId>,
    Result<Vec<u8>, DialError>,
);

fn spawn_control_dials<Dial, DialFuture, DialError>(
    (pending, dialed_peers): (&mut DialQueue, &mut BTreeSet<ma2a_core::EndpointId>),
    dials: &mut JoinSet<DialOutcome<DialError>>,
    dial: Dial,
) where
    Dial: Fn(ma2a_core::EndpointId, Vec<u8>) -> DialFuture + Clone + Send + 'static,
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

#[cfg(test)]
mod clock_tests;
#[cfg(test)]
mod deadline_tests;
#[cfg(test)]
mod tests;
