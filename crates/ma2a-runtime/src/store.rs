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
pub(crate) use membership::OwnedMemberRevocation;

pub(crate) const STORE_CAPACITY: usize = 8;
#[derive(Clone, Debug)]
pub(crate) struct StoreClient {
    pub(crate) sender: mpsc::Sender<StoreCommand>,
}

pub(crate) struct StoreBackend {
    repository: Repository,
    key_store: KeyStore,
}
