use std::{
    collections::{BTreeSet, VecDeque},
    future::Future,
};

use ma2a_net::{
    CONTROL_DIAL_CONCURRENCY, CONTROL_ROUND_DEADLINE, ControlClient, SpaceAddressLookup,
    exchange_control_with_retry,
};
use tokio::task::JoinSet;
use tokio::time::timeout;

use crate::{
    control_sync::{
        ControlApplyOutcome, ControlExchangeInput, ControlRoundOutcome, ControlRoundRequest,
        PreparedControlPeer, install_lookup,
    },
    error::RuntimeError,
    store::StoreClient,
};

pub(crate) struct ControlRoundRunner {
    store: StoreClient,
    client: ControlClient,
    lookup: SpaceAddressLookup,
}

impl ControlRoundRunner {
    pub(crate) const fn new(
        store: StoreClient,
        client: ControlClient,
        lookup: SpaceAddressLookup,
    ) -> Self {
        Self {
            store,
            client,
            lookup,
        }
    }

    pub(crate) async fn run(
        self,
        request: ControlRoundRequest,
    ) -> Result<Option<ControlRoundOutcome>, RuntimeError> {
        with_control_deadline(self.run_within_deadline(request)).await?
    }

    async fn run_within_deadline(
        self,
        request: ControlRoundRequest,
    ) -> Result<Option<ControlRoundOutcome>, RuntimeError> {
        let mut pending = self
            .store
            .prepare_control_round(request)
            .await?
            .into_iter()
            .collect::<VecDeque<_>>();
        let had_peers = !pending.is_empty();
        let mut dials = JoinSet::new();
        let mut latest = None;
        let mut synchronized_peers = BTreeSet::new();
        while !pending.is_empty() || !dials.is_empty() {
            spawn_control_dials(&mut pending, &mut dials, {
                let client = self.client.clone();
                move |peer, request| {
                    let client = client.clone();
                    async move { exchange_control_with_retry(&client, peer, &request).await }
                }
            });
            let Some(joined) = dials.join_next().await else {
                break;
            };
            let Ok(Ok((peer, response))) = joined else {
                continue;
            };
            let ControlApplyOutcome {
                revision,
                memberships,
                lookup: lookup_state,
            } = self
                .store
                .apply_control_response(ControlExchangeInput {
                    local_endpoint_id: request.local_endpoint_id,
                    remote_endpoint_id: peer,
                    payload: response,
                    now_ms: request.now_ms,
                })
                .await?;
            install_lookup(&self.lookup, lookup_state)?;
            synchronized_peers.insert(peer);
            latest = Some((revision, memberships));
        }
        if had_peers && latest.is_none() {
            return Err(RuntimeError::new(crate::error::RuntimeErrorKind::Control));
        }
        Ok(latest.map(|(revision, memberships)| ControlRoundOutcome {
            revision,
            memberships,
            synchronized_peers,
        }))
    }
}

async fn with_control_deadline<Output>(
    operation: impl Future<Output = Output>,
) -> Result<Output, RuntimeError> {
    timeout(CONTROL_ROUND_DEADLINE, operation)
        .await
        .map_err(|_| RuntimeError::new(crate::error::RuntimeErrorKind::Control))
}

fn spawn_control_dials<Dial, DialFuture, DialError>(
    pending: &mut VecDeque<PreparedControlPeer>,
    dials: &mut JoinSet<Result<(ma2a_core::EndpointId, Vec<u8>), DialError>>,
    dial: Dial,
) where
    Dial: Fn(ma2a_core::EndpointId, Vec<u8>) -> DialFuture + Clone + Send + 'static,
    DialFuture: Future<Output = Result<Vec<u8>, DialError>> + Send + 'static,
    DialError: Send + 'static,
{
    while dials.len() < CONTROL_DIAL_CONCURRENCY {
        let Some(PreparedControlPeer { peer, request }) = pending.pop_front() else {
            break;
        };
        let dial = dial.clone();
        dials.spawn(async move { dial(peer, request).await.map(|response| (peer, response)) });
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        error::Error,
        future::pending,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use ma2a_net::{CONTROL_DIAL_CONCURRENCY, EndpointSecret};
    use tokio::sync::Semaphore;

    use super::{spawn_control_dials, with_control_deadline};
    use crate::control_sync::PreparedControlPeer;

    type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

    fn prepared_peer(seed: u8) -> TestResultValue<PreparedControlPeer> {
        let peer = EndpointSecret::parse(&[seed; 32])?.endpoint_id();
        Ok(PreparedControlPeer {
            peer,
            request: vec![seed],
        })
    }

    type TestResultValue<T> = Result<T, Box<dyn Error + Send + Sync>>;

    #[tokio::test(start_paused = true)]
    async fn aggregate_round_deadline_cancels_hanging_dials() -> TestResult {
        // Given
        // When
        let task = tokio::spawn(with_control_deadline(pending::<()>()));
        tokio::task::yield_now().await;
        tokio::time::advance(ma2a_net::CONTROL_ROUND_DEADLINE).await;
        let result = task.await?;

        // Then
        assert!(result.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn dial_collection_never_exceeds_runtime_concurrency() -> TestResult {
        // Given
        let queue = (0..=CONTROL_DIAL_CONCURRENCY)
            .map(|offset| prepared_peer(0x50_u8.saturating_add(u8::try_from(offset)?)))
            .collect::<TestResultValue<VecDeque<_>>>()?;
        let gate = Arc::new(Semaphore::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(AtomicUsize::new(0));

        // When
        let mut queue = queue;
        let mut dials = tokio::task::JoinSet::new();
        spawn_control_dials(&mut queue, &mut dials, {
            let gate = Arc::clone(&gate);
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let started = Arc::clone(&started);
            move |_, _| {
                let gate = Arc::clone(&gate);
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                let started = Arc::clone(&started);
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    gate.acquire()
                        .await
                        .map_err(std::io::Error::other)?
                        .forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok::<Vec<u8>, std::io::Error>(Vec::new())
                }
            }
        });
        while started.load(Ordering::SeqCst) < CONTROL_DIAL_CONCURRENCY {
            tokio::task::yield_now().await;
        }

        // Then
        assert_eq!(started.load(Ordering::SeqCst), CONTROL_DIAL_CONCURRENCY);
        assert_eq!(maximum.load(Ordering::SeqCst), CONTROL_DIAL_CONCURRENCY);
        gate.add_permits(CONTROL_DIAL_CONCURRENCY + 1);
        let mut completed = 0;
        while let Some(joined) = dials.join_next().await {
            joined??;
            completed += 1;
            spawn_control_dials(&mut queue, &mut dials, {
                let gate = Arc::clone(&gate);
                move |_, _| {
                    let gate = Arc::clone(&gate);
                    async move {
                        gate.acquire()
                            .await
                            .map_err(std::io::Error::other)?
                            .forget();
                        Ok::<Vec<u8>, std::io::Error>(Vec::new())
                    }
                }
            });
        }
        assert_eq!(completed, CONTROL_DIAL_CONCURRENCY + 1);
        Ok(())
    }
}
