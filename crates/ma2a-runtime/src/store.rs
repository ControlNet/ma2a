mod command;
mod local_control;

use std::collections::BTreeSet;

use ma2a_net::EndpointSecret;
use ma2a_store::{
    EndpointRecord, KeyKind, KeyMaterial, KeyReference, KeyStore, Repository,
    RuntimeMetadataUpdate, StoreConfig, StoreError,
};
use tokio::sync::mpsc;

use crate::error::{RuntimeError, RuntimeErrorKind};

pub(crate) use command::StoreCommand;

pub(crate) const STORE_CAPACITY: usize = 8;
const ENDPOINT_KEY_REFERENCE: &str = "endpoint-identity-v1";

pub(crate) struct Identity {
    pub(crate) secret: EndpointSecret,
    pub(crate) endpoint_id: ma2a_core::EndpointId,
    pub(crate) memberships: BTreeSet<ma2a_core::SpaceId>,
    pub(crate) bind_port: Option<u16>,
}

#[derive(Clone, Debug)]
pub(crate) struct StoreClient {
    pub(crate) sender: mpsc::Sender<StoreCommand>,
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

    #[expect(
        clippy::too_many_lines,
        reason = "single dispatcher preserves store command ordering"
    )]
    pub(crate) fn run(mut self, mut commands: mpsc::Receiver<StoreCommand>) {
        while let Some(command) = commands.blocking_recv() {
            match command {
                StoreCommand::Initialize(reply) => {
                    let _unsent = reply.send(self.initialize());
                }
                StoreCommand::SetEndpointBindPort { port, reply } => {
                    let _unsent = reply.send(self.set_endpoint_bind_port(port));
                }
                StoreCommand::BeginBoot {
                    boot_id,
                    observed_at_ms,
                    reply,
                } => {
                    let _unsent = reply.send(self.record_metadata(RuntimeMetadataUpdate {
                        boot_id,
                        last_shutdown_clean: false,
                        observed_at_ms,
                    }));
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
                    let _unsent = reply.send(self.record_metadata(RuntimeMetadataUpdate {
                        boot_id,
                        last_shutdown_clean: true,
                        observed_at_ms,
                    }));
                }
                StoreCommand::CreateEnrollmentInvite {
                    creation,
                    creator,
                    owner_addr,
                    reply,
                } => {
                    let result = self.repository.create_enrollment_invite(
                        creation.space_id,
                        creator,
                        owner_addr,
                        creation.validity,
                        &creation.entropy,
                    );
                    let _unsent = reply.send(result.map_err(Into::into));
                }
                StoreCommand::CancelEnrollmentInvite {
                    invitation_id,
                    reply,
                } => {
                    let _unsent = reply.send(
                        self.repository
                            .cancel_invitation(invitation_id)
                            .map_err(Into::into),
                    );
                }
                StoreCommand::RedeemEnrollment { authorized, reply } => {
                    let _unsent = reply.send(
                        self.repository
                            .redeem_enrollment(&authorized)
                            .map_err(Into::into),
                    );
                }
                StoreCommand::PersistEnrollment { chain, reply } => {
                    let _unsent = reply.send(self.persist_enrollment(chain));
                }
                StoreCommand::AdvanceOwnedSpace {
                    update,
                    local_endpoint_id,
                    reply,
                } => {
                    let result =
                        self.repository
                            .advance_owned_space(&update)
                            .and_then(|advanced| {
                                Ok((
                                    advanced.revision(),
                                    self.repository.memberships_for(local_endpoint_id)?,
                                ))
                            });
                    let _unsent = reply.send(result.map_err(Into::into));
                }
                StoreCommand::PublishAddress {
                    publisher,
                    local_endpoint_id,
                    now_ms,
                    reply,
                } => {
                    let _unsent =
                        reply.send(self.publish_address(&publisher, local_endpoint_id, now_ms));
                }
                StoreCommand::PublishRelayAdvertisements {
                    publisher,
                    local_endpoint_id,
                    issued_at_ms,
                    expires_at_ms,
                    reply,
                } => {
                    let _unsent = reply.send(self.publish_relay_advertisements(
                        &publisher,
                        local_endpoint_id,
                        issued_at_ms,
                        expires_at_ms,
                    ));
                }
                StoreCommand::LoadControlLookup {
                    local_endpoint_id,
                    now_ms,
                    reply,
                } => {
                    let result = crate::control_sync::load_lookup(
                        &mut self.repository,
                        local_endpoint_id,
                        now_ms,
                    );
                    let _unsent = reply.send(result);
                }
                StoreCommand::PrepareControlRound { input, reply } => {
                    let result = crate::control_sync::prepare_round(&self.repository, &input);
                    let _unsent = reply.send(result);
                }
                StoreCommand::RespondControl { input, reply } => {
                    let result = crate::control_sync::respond(&mut self.repository, &input);
                    let _unsent = reply.send(result);
                }
                StoreCommand::ApplyControlResponse { input, reply } => {
                    let result = crate::control_sync::apply_response(&mut self.repository, &input);
                    let _unsent = reply.send(result);
                }
                StoreCommand::Stop(reply) => {
                    let _unsent = reply.send(());
                    break;
                }
            }
        }
    }

    fn set_endpoint_bind_port(&mut self, port: u16) -> Result<u64, RuntimeError> {
        self.repository
            .set_endpoint_bind_port(port)
            .map_err(Into::into)
    }

    fn record_metadata(&mut self, update: RuntimeMetadataUpdate) -> Result<u64, RuntimeError> {
        self.repository
            .record_runtime_metadata(&update)
            .map_err(Into::into)
    }

    fn persist_enrollment(
        &mut self,
        chain: ma2a_core::SpaceChain,
    ) -> Result<(u64, ma2a_core::SpaceChain), RuntimeError> {
        let revision = self
            .repository
            .persist_space_chain(&chain)?
            .revision()
            .map_or_else(|| self.repository.revision(), Ok)?;
        Ok((revision, chain))
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
        let memberships = self.repository.memberships_for(endpoint_id)?;
        let bind_port = self.repository.endpoint_bind_port()?;
        Ok(Identity {
            secret,
            endpoint_id,
            memberships,
            bind_port,
        })
    }
}

pub(crate) fn channel_error<T>(_error: T) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::Channel)
}

pub(crate) fn count(value: usize) -> Result<u64, RuntimeError> {
    u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::ObservationOverflow))
}
