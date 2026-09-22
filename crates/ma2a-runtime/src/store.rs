mod backend_state;
mod command;
mod identity;
mod local_control;
mod membership;
mod mutation_replay;
mod relay_state;
mod runner;

use ma2a_store::{KeyStore, Repository};
use tokio::sync::mpsc;

pub(crate) use crate::store_client::channel_error;

pub(crate) use command::StoreCommand;
pub(crate) use identity::Identity;
pub(crate) use membership::{OwnedMemberRevocation, RemovedMember};

/// Everything one enrollment commit needs from the caller.
pub(crate) struct EnrollmentPersistence {
    pub(crate) chain: ma2a_core::SpaceChain,
    pub(crate) owner_address: Box<ma2a_store::ValidatedAddressRecord>,
    pub(crate) local_endpoint_id: ma2a_core::EndpointId,
}

/// Durable outcome of one enrollment, including the membership the Store holds.
pub(crate) struct PersistedEnrollment {
    pub(crate) revision: u64,
    pub(crate) chain: ma2a_core::SpaceChain,
    pub(crate) memberships: std::collections::BTreeSet<ma2a_core::SpaceId>,
}

pub(crate) const STORE_CAPACITY: usize = 8;
#[derive(Clone, Debug)]
pub(crate) struct StoreClient {
    pub(crate) sender: mpsc::Sender<StoreCommand>,
}

pub(crate) struct StoreBackend {
    repository: Repository,
    key_store: KeyStore,
}
