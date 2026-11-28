use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use crate::control_sync::ControlFailure;
use ma2a_core::EndpointId;
use tokio::sync::oneshot;

use crate::{
    control_sync::{
        ControlApplyOutcome, ControlAuthorizationInput, ControlExchangeInput, ControlRespondOutcome,
    },
    store::{StoreClient, StoreCommand},
    store_client::channel_error,
};

const APPLY_QUEUED: u8 = 0;
const APPLY_STARTED: u8 = 1;
const APPLY_CANCELLED: u8 = 2;

/// Shared atomic state coordinating whether a queued control apply has started.
#[derive(Clone, Debug)]
pub(crate) struct ControlApplyLease(Arc<AtomicU8>);

impl ControlApplyLease {
    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicU8::new(APPLY_QUEUED)))
    }

    /// Claims execution on the Store thread; returns `false` if already cancelled.
    pub(crate) fn begin(&self) -> bool {
        self.0
            .compare_exchange(
                APPLY_QUEUED,
                APPLY_STARTED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Cancels a still-queued apply; returns `false` if the Store already started it.
    pub(crate) fn cancel(&self) -> bool {
        match self.0.compare_exchange(
            APPLY_QUEUED,
            APPLY_CANCELLED,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => true,
            Err(current) => current == APPLY_CANCELLED,
        }
    }
}

pub(crate) struct AdmittedControlApply {
    pub(crate) response: oneshot::Receiver<Result<ControlApplyOutcome, ControlFailure>>,
    pub(crate) lease: ControlApplyLease,
}

impl StoreClient {
    pub(crate) async fn authorize_control(
        &self,
        local_endpoint_id: EndpointId,
        peer_endpoint_id: EndpointId,
    ) -> Result<(), ControlFailure> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::AuthorizeControl {
                input: ControlAuthorizationInput {
                    local_endpoint_id,
                    remote_endpoint_id: peer_endpoint_id,
                },
                reply,
            })
            .await
            .map_err(channel_error)?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn respond_control(
        &self,
        input: ControlExchangeInput,
    ) -> Result<ControlRespondOutcome, ControlFailure> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(StoreCommand::RespondControl { input, reply })
            .await
            .map_err(channel_error)?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn admit_control_response(
        &self,
        (input, requested): (ControlExchangeInput, Vec<ma2a_core::SpaceId>),
        deadline: tokio::time::Instant,
    ) -> Result<AdmittedControlApply, ControlFailure> {
        let permit = self.sender.reserve().await.map_err(channel_error)?;
        let (reply, response) = oneshot::channel();
        let lease = ControlApplyLease::new();
        permit.send(StoreCommand::ApplyControlResponse {
            input: (input, requested),
            deadline,
            lease: lease.clone(),
            reply,
        });
        Ok(AdmittedControlApply { response, lease })
    }
}
