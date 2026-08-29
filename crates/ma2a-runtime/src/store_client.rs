use ma2a_store::{EndpointObservationUpdate, EnrollmentOutcome, EnrollmentRedemption};
use tokio::sync::oneshot;

use crate::{
    error::RuntimeError,
    state::RuntimeStatus,
    store::{Identity, StoreClient, StoreCommand, channel_error, count, now_ms},
};

impl StoreClient {
    pub(crate) const fn new(sender: tokio::sync::mpsc::Sender<StoreCommand>) -> Self {
        Self { sender }
    }

    pub(crate) async fn initialize(&self) -> Result<Identity, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::Initialize(reply)).await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn begin_boot(&self, boot_id: [u8; 16]) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::BeginBoot {
            boot_id,
            observed_at_ms: now_ms()?,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn observe(&self, state: &RuntimeStatus) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        let observation = EndpointObservationUpdate {
            observed_at_ms: now_ms()?,
            ready: state.ready,
            direct_address_count: count(state.endpoint_addr.ip_addrs().count())?,
            relay_address_count: count(state.endpoint_addr.relay_urls().count())?,
            membership_count: count(state.membership_count())?,
        };
        self.send(StoreCommand::Observe { observation, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn clean_shutdown(&self, boot_id: [u8; 16]) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::CleanShutdown {
            boot_id,
            observed_at_ms: now_ms()?,
            reply,
        })
        .await?;
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
        creation: crate::EnrollmentCreation,
        creator: ma2a_core::EndpointId,
        owner_addr: ma2a_net::EndpointAddr,
    ) -> Result<ma2a_core::SignedInviteTicket, RuntimeError> {
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
        ticket: ma2a_core::SignedInviteTicket,
        redemption: EnrollmentRedemption,
    ) -> Result<EnrollmentOutcome, RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::RedeemEnrollment {
            ticket,
            redemption,
            reply,
        })
        .await?;
        response.await.map_err(channel_error)?
    }

    pub(crate) async fn persist_enrollment(
        &self,
        chain: Vec<u8>,
    ) -> Result<(u64, ma2a_core::SpaceChain), RuntimeError> {
        let (reply, response) = oneshot::channel();
        self.send(StoreCommand::PersistEnrollment { chain, reply })
            .await?;
        response.await.map_err(channel_error)?
    }

    async fn send(&self, command: StoreCommand) -> Result<(), RuntimeError> {
        self.sender.send(command).await.map_err(channel_error)
    }
}
