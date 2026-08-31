mod command;
mod identity;
mod local_control;
mod membership;
mod relay_state;

use ma2a_store::{KeyStore, Repository, RuntimeMetadataUpdate};
use tokio::sync::mpsc;

use crate::error::{RuntimeError, RuntimeErrorKind};

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

impl StoreBackend {
    #[expect(clippy::too_many_lines, reason = "store command ordering is explicit")]
    pub(crate) fn run(mut self, mut commands: mpsc::Receiver<StoreCommand>) {
        while let Some(command) = commands.blocking_recv() {
            match command {
                StoreCommand::Initialize(reply) => {
                    let _unsent = reply.send(self.initialize());
                }
                StoreCommand::Snapshot {
                    endpoint_id,
                    now_ms,
                    reply,
                } => {
                    let result = self.repository.snapshot_state(endpoint_id, now_ms);
                    let _unsent = reply.send(result.map_err(Into::into));
                }
                StoreCommand::CreateOwnedSpace { creation, reply } => {
                    let result = self
                        .repository
                        .create_owned_space(&creation)
                        .map_err(Into::into);
                    let _unsent = reply.send(result);
                }
                StoreCommand::RevokeOwnedSpaceMember { request, reply } => {
                    let _unsent = reply.send(self.revoke_owned_space_member(request));
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
                    force_advance,
                    reply,
                } => {
                    let _unsent = reply.send(self.publish_address(
                        &publisher,
                        local_endpoint_id,
                        now_ms,
                        force_advance,
                    ));
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
                StoreCommand::LoadRelayMap {
                    local_endpoint_id,
                    now_ms,
                    reply,
                } => {
                    let _unsent = reply.send(self.load_relay_map(local_endpoint_id, now_ms));
                }
                StoreCommand::RecordRelayObservations {
                    observations,
                    reply,
                } => {
                    let _unsent = reply.send(self.record_relay_observations(&observations));
                }
                StoreCommand::RelayConfiguration { reply } => {
                    self.reply_relay_configuration(reply);
                }
                StoreCommand::SetRelayConfiguration {
                    configuration,
                    reply,
                } => {
                    self.reply_set_relay_configuration(&configuration, reply);
                }
                StoreCommand::RelayAuthorizations {
                    local_endpoint_id,
                    reply,
                } => {
                    self.reply_relay_authorizations(local_endpoint_id, reply);
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
                StoreCommand::AuthorizeEcho {
                    local_endpoint_id,
                    peer_endpoint_id,
                    reply,
                } => {
                    let result = crate::services::echo::authorize(
                        &self.repository,
                        local_endpoint_id,
                        peer_endpoint_id,
                    );
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
}

pub(crate) fn channel_error<T>(_error: T) -> RuntimeError {
    RuntimeError::new(RuntimeErrorKind::Channel)
}
