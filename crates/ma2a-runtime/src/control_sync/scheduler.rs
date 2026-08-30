use std::collections::VecDeque;

use ma2a_net::{
    CONTROL_DIAL_CONCURRENCY, ControlClient, SpaceAddressLookup, exchange_control_with_retry,
};
use tokio::task::JoinSet;

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
        let mut pending = self
            .store
            .prepare_control_round(request)
            .await?
            .into_iter()
            .collect::<VecDeque<_>>();
        let had_peers = !pending.is_empty();
        let mut dials = JoinSet::new();
        let mut latest = None;
        while !pending.is_empty() || !dials.is_empty() {
            while dials.len() < CONTROL_DIAL_CONCURRENCY {
                let Some(PreparedControlPeer { peer, request }) = pending.pop_front() else {
                    break;
                };
                let client = self.client.clone();
                dials.spawn(async move {
                    exchange_control_with_retry(&client, peer, &request)
                        .await
                        .map(|response| (peer, response))
                });
            }
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
            latest = Some((revision, memberships));
        }
        if had_peers && latest.is_none() {
            return Err(RuntimeError::new(crate::error::RuntimeErrorKind::Control));
        }
        Ok(latest.map(|(revision, memberships)| ControlRoundOutcome {
            revision,
            memberships,
        }))
    }
}
