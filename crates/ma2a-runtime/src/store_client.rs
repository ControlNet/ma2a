use ma2a_store::{AuthorizedEnrollmentRedemption, EnrollmentOutcome};
use tokio::sync::oneshot;

mod echo;
mod membership;
mod observation;
mod relay;
mod snapshot;

use crate::{
    RuntimeClock as _,
    clock::SystemClock,
    control_sync::{
        ControlApplyOutcome, ControlLookupState, ControlRespondOutcome, PreparedControlPeer,
    },
    error::{RuntimeError, RuntimeErrorKind},
    store::{Identity, StoreClient, StoreCommand},
};

pub(crate) fn channel_error<T>(_error: T) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::Channel)
}

impl StoreClient {
    pub(crate) const fn new(sender: tokio::sync::mpsc::Sender<StoreCommand>) -> Self {
        Self { sender }
    }

    pub(crate) async fn initialize(&self) -> Result<Identity, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::Initialize(reply)).await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn set_endpoint_bind_port(&self, port: u16) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::SetEndpointBindPort { port, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn begin_boot(&self, boot_id: [u8; 16]) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::BeginBoot {
            boot_id,
            observed_at_ms: SystemClock.now_ms()?,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn clean_shutdown(&self, boot_id: [u8; 16]) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::CleanShutdown {
            boot_id,
            observed_at_ms: SystemClock.now_ms()?,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn advance_revision(&self) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::AdvanceRevision(reply)).await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn stop(&self) -> Result<(), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::Stop(reply)).await?;
        response.await.map_err(channel_error)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the store command carries creation, authenticated creator, and owner address"
    )]
    pub(crate) async fn create_enrollment_invite(
        &self,
        creation: crate::enrollment::IssuedEnrollmentCreation,
        creator: ma2a_core::EndpointId,
        owner_addr: ma2a_net::EndpointAddr,
    ) -> Result<ma2a_store::CreatedEnrollmentInvite, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::CreateEnrollmentInvite {
            creation,
            creator,
            owner_addr,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn cancel_enrollment_invite(
        &self,
        invitation_id: [u8; 16],
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::CancelEnrollmentInvite {
            invitation_id,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn redeem_enrollment(
        &self,
        authorized: AuthorizedEnrollmentRedemption,
    ) -> Result<EnrollmentOutcome, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RedeemEnrollment { authorized, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn persist_enrollment(
        &self,
        chain: ma2a_core::SpaceChain,
    ) -> Result<(u64, ma2a_core::SpaceChain), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::PersistEnrollment { chain, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn advance_owned_space(
        &self,
        update: ma2a_store::OwnedSpaceUpdate,
        local_endpoint_id: ma2a_core::EndpointId,
    ) -> Result<(u64, std::collections::BTreeSet<ma2a_core::SpaceId>), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::AdvanceOwnedSpace {
            update,
            local_endpoint_id,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mailbox method mirrors its command payload"
    )]
    pub(crate) async fn publish_address(
        &self,
        publisher: ma2a_net::AddressPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
        force_advance: bool,
    ) -> Result<(u64, bool), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::PublishAddress {
            publisher,
            local_endpoint_id,
            now_ms,
            force_advance,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "mailbox method mirrors its command payload"
    )]
    pub(crate) async fn publish_relay_advertisements(
        &self,
        publisher: ma2a_net::PrivateRelayAdvertisementPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        issued_at_ms: u64,
        expires_at_ms: u64,
    ) -> Result<(u64, bool), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::PublishRelayAdvertisements {
            publisher,
            local_endpoint_id,
            issued_at_ms,
            expires_at_ms,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn load_control_lookup(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
    ) -> Result<ControlLookupState, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::LoadControlLookup {
            local_endpoint_id,
            now_ms,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn load_relay_map(
        &self,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
    ) -> Result<ma2a_net::LocalIrohRelayMap, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::LoadRelayMap {
            local_endpoint_id,
            now_ms,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn record_relay_observations(
        &self,
        observations: Vec<ma2a_store::RelayObservation>,
    ) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RecordRelayObservations {
            observations,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn prepare_control_round(
        &self,
        input: crate::control_sync::ControlRoundRequest,
    ) -> Result<Vec<PreparedControlPeer>, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::PrepareControlRound { input, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn respond_control(
        &self,
        input: crate::control_sync::ControlExchangeInput,
    ) -> Result<ControlRespondOutcome, ma2a_net::ControlRejection> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RespondControl { input, reply })
            .await
            .map_err(|_| ma2a_net::ControlRejection::Unavailable)?;
        response
            .await
            .map_err(|_| ma2a_net::ControlRejection::Unavailable)?
    }

    pub(crate) async fn apply_control_response(
        &self,
        input: crate::control_sync::ControlExchangeInput,
    ) -> Result<ControlApplyOutcome, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::ApplyControlResponse { input, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    async fn send(&self, command: StoreCommand) -> Result<(), RuntimeError> {
        self.sender.send(command).await.map_err(channel_error)
    }
}
