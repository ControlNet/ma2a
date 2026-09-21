use ma2a_core::{EchoError, EchoResponse, EndpointId, RequestId, SignedInviteTicket, SpaceId};
use tokio::sync::oneshot;

use super::ShutdownAck;
use crate::{
    EnrollmentAttempt, EnrollmentCreation, EnrollmentError, EstablishedEnrollment,
    error::RuntimeError, state::RuntimeStatus,
};

pub(crate) enum Command {
    SpaceDetails(
        ma2a_core::SpaceId,
        oneshot::Sender<Result<Option<crate::api::SpaceDetailsView>, RuntimeError>>,
    ),
    SnapshotStamp(oneshot::Sender<Result<crate::api::SnapshotStampView, RuntimeError>>),
    Status(oneshot::Sender<RuntimeStatus>),
    Snapshot(oneshot::Sender<Result<crate::api::RuntimeSnapshot, RuntimeError>>),
    ObserveMemberships {
        memberships: Vec<SpaceId>,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    CreateOwnedSpace {
        name: String,
        reply: oneshot::Sender<Result<SpaceId, RuntimeError>>,
    },
    RevokeOwnedSpaceMember {
        space_id: SpaceId,
        endpoint_id: EndpointId,
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
    RelayConfiguration {
        reply: oneshot::Sender<Result<ma2a_store::RelayConfiguration, RuntimeError>>,
    },
    RelayStatus {
        reply: oneshot::Sender<Result<super::RelayRuntimeStatus, RuntimeError>>,
    },
    SetRelayConfiguration {
        configuration: ma2a_store::RelayConfiguration,
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
