use ma2a_store::{AuthorizedEnrollmentRedemption, EndpointObservationUpdate, EnrollmentOutcome};
use tokio::sync::oneshot;

use crate::{
    control_sync::{
        ControlApplyOutcome, ControlAuthorizationInput, ControlExchangeInput, ControlLookupState,
        ControlRespondOutcome, ControlRoundRequest, PreparedControlPeer,
    },
    error::RuntimeError,
};

pub(crate) enum StoreCommand {
    Initialize(oneshot::Sender<Result<super::Identity, RuntimeError>>),
    Revision(oneshot::Sender<Result<u64, RuntimeError>>),
    SpaceDetails {
        endpoint_id: ma2a_core::EndpointId,
        space_id: ma2a_core::SpaceId,
        reply: oneshot::Sender<Result<Option<ma2a_store::SpaceDetails>, RuntimeError>>,
    },
    Snapshot {
        endpoint_id: ma2a_core::EndpointId,
        now_ms: i64,
        reply: oneshot::Sender<Result<ma2a_store::SnapshotState, RuntimeError>>,
    },
    MutationReplay {
        request_id: ma2a_core::RequestId,
        reply: oneshot::Sender<Result<Option<ma2a_store::MutationReplayState>, RuntimeError>>,
    },
    ReserveMutationReplay {
        request_id: ma2a_core::RequestId,
        fingerprint: [u8; 32],
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    AbortMutationReplay {
        request_id: ma2a_core::RequestId,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    RecordMutationReplay {
        record: ma2a_store::MutationReplayRecord,
        reply: oneshot::Sender<Result<(), RuntimeError>>,
    },
    CreateOwnedSpace {
        creation: ma2a_store::SpaceCreation,
        reply: oneshot::Sender<Result<ma2a_store::CreatedSpace, RuntimeError>>,
    },
    RevokeOwnedSpaceMember {
        request: super::OwnedMemberRevocation,
        reply: oneshot::Sender<
            Result<(u64, std::collections::BTreeSet<ma2a_core::SpaceId>), RuntimeError>,
        >,
    },
    SetEndpointBindPort {
        port: u16,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    BeginBoot {
        boot_id: [u8; 16],
        observed_at_ms: i64,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    Observe {
        observation: EndpointObservationUpdate,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    CleanShutdown {
        boot_id: [u8; 16],
        observed_at_ms: i64,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    CreateEnrollmentInvite {
        creation: crate::enrollment::IssuedEnrollmentCreation,
        creator: ma2a_core::EndpointId,
        owner_addr: ma2a_net::EndpointAddr,
        reply: oneshot::Sender<Result<ma2a_store::CreatedEnrollmentInvite, RuntimeError>>,
    },
    CancelEnrollmentInvite {
        invitation_id: [u8; 16],
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    RedeemEnrollment {
        authorized: AuthorizedEnrollmentRedemption,
        reply: oneshot::Sender<Result<EnrollmentOutcome, RuntimeError>>,
    },
    PersistEnrollment {
        chain: ma2a_core::SpaceChain,
        owner_address: Box<ma2a_store::ValidatedAddressRecord>,
        reply: oneshot::Sender<Result<(u64, ma2a_core::SpaceChain), RuntimeError>>,
    },
    AddressRecord {
        space_id: ma2a_core::SpaceId,
        endpoint_id: ma2a_core::EndpointId,
        reply: oneshot::Sender<Result<Option<ma2a_store::PersistedAddressRecord>, RuntimeError>>,
    },
    AdvanceOwnedSpace {
        update: ma2a_store::OwnedSpaceUpdate,
        local_endpoint_id: ma2a_core::EndpointId,
        reply: oneshot::Sender<
            Result<(u64, std::collections::BTreeSet<ma2a_core::SpaceId>), RuntimeError>,
        >,
    },
    PublishAddress {
        publisher: ma2a_net::AddressPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
        force_advance: bool,
        reply: oneshot::Sender<Result<(u64, bool), RuntimeError>>,
    },
    PublishRelayAdvertisements {
        publisher: ma2a_net::PrivateRelayAdvertisementPublisher,
        local_endpoint_id: ma2a_core::EndpointId,
        issued_at_ms: u64,
        expires_at_ms: u64,
        reply: oneshot::Sender<Result<(u64, bool), RuntimeError>>,
    },
    ReconcileRelayActivity {
        local_endpoint_id: ma2a_core::EndpointId,
        active_spaces: Vec<ma2a_core::SpaceId>,
        reply: oneshot::Sender<Result<(u64, bool), RuntimeError>>,
    },
    LoadControlLookup {
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
        reply: oneshot::Sender<Result<ControlLookupState, RuntimeError>>,
    },
    LoadRelayMap {
        local_endpoint_id: ma2a_core::EndpointId,
        now_ms: u64,
        reply: oneshot::Sender<Result<ma2a_net::LocalIrohRelayMap, RuntimeError>>,
    },
    RecordRelayObservations {
        observations: Vec<ma2a_store::RelayObservation>,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    RelayConfiguration {
        reply: oneshot::Sender<Result<ma2a_store::RelayConfiguration, RuntimeError>>,
    },
    SetRelayConfiguration {
        configuration: ma2a_store::RelayConfiguration,
        reply: oneshot::Sender<Result<u64, RuntimeError>>,
    },
    RelayAuthorizations {
        local_endpoint_id: ma2a_core::EndpointId,
        reply: oneshot::Sender<Result<Vec<ma2a_core::SpaceAuthorizationView>, RuntimeError>>,
    },
    PrepareControlRound {
        input: ControlRoundRequest,
        reply: oneshot::Sender<Result<Vec<PreparedControlPeer>, RuntimeError>>,
    },
    RespondControl {
        input: ControlExchangeInput,
        reply: oneshot::Sender<Result<ControlRespondOutcome, ma2a_net::ControlRejection>>,
    },
    AuthorizeControl {
        input: ControlAuthorizationInput,
        reply: oneshot::Sender<Result<(), ma2a_net::ControlRejection>>,
    },
    ApplyControlResponse {
        input: ControlExchangeInput,
        reply: oneshot::Sender<Result<ControlApplyOutcome, RuntimeError>>,
    },
    AuthorizeEcho {
        local_endpoint_id: ma2a_core::EndpointId,
        peer_endpoint_id: ma2a_core::EndpointId,
        reply: oneshot::Sender<Result<(), ma2a_core::EchoError>>,
    },
    AdvanceRevision(oneshot::Sender<Result<u64, RuntimeError>>),
    Stop(oneshot::Sender<()>),
}
