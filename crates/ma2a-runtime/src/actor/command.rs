use ma2a_core::{EchoError, EchoResponse, EndpointId, RequestId, SignedInviteTicket, SpaceId};
use tokio::sync::oneshot;

use super::ShutdownAck;
use crate::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentError, EstablishedEnrollment,
    error::RuntimeError, state::RuntimeStatus,
};

pub(crate) enum Command {
    Status(oneshot::Sender<RuntimeStatus>),
    ObserveMemberships {
        memberships: Vec<SpaceId>,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    CreateEnrollmentInvite {
        creation: EnrollmentCreation,
        reply: oneshot::Sender<Result<SignedInviteTicket, EnrollmentError>>,
    },
    RedeemEnrollment {
        attempt: Box<EnrollmentAttempt>,
        reply: oneshot::Sender<Result<EstablishedEnrollment, EnrollmentError>>,
    },
    CancelEnrollmentInvite {
        invitation_id: [u8; 16],
        reply: oneshot::Sender<Result<(), EnrollmentError>>,
    },
    AdoptRevision {
        revision: u64,
        reply: oneshot::Sender<u64>,
    },
    ControlSyncStatus {
        peer: ma2a_core::EndpointId,
        reply: oneshot::Sender<bool>,
    },
    SyncControl {
        peer: Option<ma2a_core::EndpointId>,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    AdvanceOwnedSpace {
        update: ma2a_store::OwnedSpaceUpdate,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    PublishAddress(oneshot::Sender<Result<u64, RuntimeError>>),
    PublishRelayAdvertisements {
        config: ma2a_net::PrivateRelayProviderConfig,
        expires_at_ms: u64,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    Echo {
        request_id: RequestId,
        target: EndpointId,
        payload: Vec<u8>,
        reply: oneshot::Sender<Result<EchoResponse<'static>, EchoError>>,
    },
    Shutdown(oneshot::Sender<ShutdownAck>),
}
