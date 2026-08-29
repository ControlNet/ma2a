use std::time::{SystemTime, UNIX_EPOCH};

use ma2a_net::EndpointSecret;
use ma2a_store::{
    EndpointObservationUpdate, EndpointRecord, KeyKind, KeyMaterial, KeyReference, KeyStore,
    Repository, RuntimeMetadataUpdate, StoreConfig, StoreError,
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    error::{RuntimeError, RuntimeErrorKind},
    state::RuntimeStatus,
};

pub(crate) const STORE_CAPACITY: usize = 8;
const ENDPOINT_KEY_REFERENCE: &str = "endpoint-identity-v1";

pub(crate) enum StoreCommand {
    Initialize(oneshot::Sender<Result<Identity, RuntimeError>>),
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
    Stop(oneshot::Sender<()>),
}

pub(crate) struct Identity {
    pub(crate) secret: EndpointSecret,
    pub(crate) endpoint_id: ma2a_core::EndpointId,
}

#[derive(Clone, Debug)]
pub(crate) struct StoreClient {
    sender: mpsc::Sender<StoreCommand>,
}

impl StoreClient {
    pub(crate) const fn new(sender: mpsc::Sender<StoreCommand>) -> Self {
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

    async fn send(&self, command: StoreCommand) -> Result<(), RuntimeError> {
        self.sender.send(command).await.map_err(channel_error)
    }
}

pub(crate) struct StoreBackend {
    repository: Repository,
    key_store: KeyStore,
}

impl StoreBackend {
    pub(crate) fn open(config: &StoreConfig) -> Result<Self, RuntimeError> {
        Ok(Self {
            repository: Repository::open(config)?,
            key_store: KeyStore::open(config.state_dir())?,
        })
    }

    pub(crate) fn run(mut self, mut commands: mpsc::Receiver<StoreCommand>) {
        while let Some(command) = commands.blocking_recv() {
            match command {
                StoreCommand::Initialize(reply) => {
                    let _unsent = reply.send(self.initialize());
                }
                StoreCommand::BeginBoot {
                    boot_id,
                    observed_at_ms,
                    reply,
                } => {
                    let result = self
                        .repository
                        .record_runtime_metadata(&RuntimeMetadataUpdate {
                            boot_id,
                            last_shutdown_clean: false,
                            observed_at_ms,
                        });
                    let _unsent = reply.send(result.map_err(Into::into));
                }
                StoreCommand::Observe { observation, reply } => {
                    let result = self.repository.record_endpoint_observation(&observation);
                    let _unsent = reply.send(result.map_err(Into::into));
                }
                StoreCommand::CleanShutdown {
                    boot_id,
                    observed_at_ms,
                    reply,
                } => {
                    let result = self
                        .repository
                        .record_runtime_metadata(&RuntimeMetadataUpdate {
                            boot_id,
                            last_shutdown_clean: true,
                            observed_at_ms,
                        });
                    let _unsent = reply.send(result.map_err(Into::into));
                }
                StoreCommand::Stop(reply) => {
                    let _unsent = reply.send(());
                    break;
                }
            }
        }
    }

    fn initialize(&mut self) -> Result<Identity, RuntimeError> {
        let reference = KeyReference::parse(ENDPOINT_KEY_REFERENCE)?;
        let record = self.repository.endpoint()?;
        if let Some(endpoint) = &record
            && endpoint.key_reference() != &reference
        {
            return Err(RuntimeError::new(RuntimeErrorKind::KeyReferenceMismatch));
        }
        let secret = match self.key_store.read(KeyKind::Endpoint, &reference) {
            Ok(protected) => EndpointSecret::parse(protected.as_ref())?,
            Err(StoreError::MissingProtectedKey { .. }) if record.is_none() => {
                let secret = EndpointSecret::generate();
                let bytes = secret.protected_bytes();
                self.key_store.write(KeyMaterial::new(
                    KeyKind::Endpoint,
                    &reference,
                    bytes.as_ref(),
                ))?;
                secret
            }
            Err(error) => return Err(error.into()),
        };
        let endpoint_id = secret.endpoint_id();
        if let Some(endpoint) = record {
            if endpoint.endpoint_id() != endpoint_id {
                return Err(RuntimeError::new(RuntimeErrorKind::IdentityMismatch));
            }
        } else {
            self.repository
                .set_endpoint(&EndpointRecord::new(endpoint_id, reference))?;
        }
        Ok(Identity {
            secret,
            endpoint_id,
        })
    }
}

fn channel_error<T>(_error: T) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::Channel)
}

fn now_ms() -> Result<i64, RuntimeError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))?
        .as_millis();
    i64::try_from(millis).map_err(|_| RuntimeError::new(RuntimeErrorKind::Clock))
}

fn count(value: usize) -> Result<u64, RuntimeError> {
    u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::ObservationOverflow))
}
