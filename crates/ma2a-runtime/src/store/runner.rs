use ma2a_store::RuntimeMetadataUpdate;
use tokio::sync::mpsc;

use super::{StoreBackend, StoreCommand, local_control, mutation_replay};

impl StoreBackend {
    pub(crate) fn run(mut self, mut commands: mpsc::Receiver<StoreCommand>) {
        while let Some(command) = commands.blocking_recv() {
            if self.dispatch(command) {
                break;
            }
        }
    }

    #[expect(clippy::too_many_lines, reason = "store command ordering is explicit")]
    fn dispatch(&mut self, command: StoreCommand) -> bool {
        match command {
            StoreCommand::Initialize(reply) => drop(reply.send(self.initialize())),
            StoreCommand::Revision(reply) => {
                drop(reply.send(self.repository.revision().map_err(Into::into)));
            }
            StoreCommand::SpaceDetails {
                endpoint_id,
                space_id,
                reply,
            } => {
                drop(
                    reply.send(
                        self.repository
                            .space_details(endpoint_id, space_id)
                            .map_err(Into::into),
                    ),
                );
            }
            StoreCommand::Snapshot {
                endpoint_id,
                now_ms,
                reply,
            } => {
                let result = self.repository.snapshot_state(endpoint_id, now_ms);
                drop(reply.send(result.map_err(Into::into)));
            }
            StoreCommand::MutationReplay { request_id, reply } => {
                mutation_replay::lookup(&self.repository, request_id, reply);
            }
            StoreCommand::ReserveMutationReplay {
                request_id,
                fingerprint,
                reply,
            } => mutation_replay::reserve(
                &mut self.repository,
                ma2a_store::MutationReplayRequest::new(request_id, fingerprint),
                reply,
            ),
            StoreCommand::AbortMutationReplay { request_id, reply } => {
                mutation_replay::abort(&mut self.repository, request_id, reply);
            }
            StoreCommand::RecordMutationReplay { record, reply } => {
                mutation_replay::complete(&mut self.repository, &record, reply);
            }
            StoreCommand::CreateOwnedSpace { creation, reply } => drop(
                reply.send(
                    self.repository
                        .create_owned_space(&creation)
                        .map_err(Into::into),
                ),
            ),
            StoreCommand::RevokeOwnedSpaceMember { request, reply } => {
                drop(reply.send(self.revoke_owned_space_member(request)));
            }
            StoreCommand::Memberships {
                local_endpoint_id,
                reply,
            } => drop(reply.send(self.memberships(local_endpoint_id))),
            StoreCommand::LoadSpaceChain { space_id, reply } => {
                drop(reply.send(self.load_space_chain(space_id)));
            }
            StoreCommand::PersistDeparture {
                chain,
                local_endpoint_id,
                reply,
            } => {
                drop(reply.send(self.persist_departure(&chain, local_endpoint_id)));
            }
            StoreCommand::SetEndpointBindPort { port, reply } => {
                drop(reply.send(self.set_endpoint_bind_port(port)));
            }
            StoreCommand::BeginBoot {
                boot_id,
                observed_at_ms,
                reply,
            } => drop(reply.send(self.record_metadata(RuntimeMetadataUpdate {
                boot_id,
                last_shutdown_clean: false,
                observed_at_ms,
            }))),
            StoreCommand::Observe { observation, reply } => drop(
                reply.send(
                    self.repository
                        .record_endpoint_observation(&observation)
                        .map_err(Into::into),
                ),
            ),
            StoreCommand::CleanShutdown {
                boot_id,
                observed_at_ms,
                reply,
            } => drop(reply.send(self.record_metadata(RuntimeMetadataUpdate {
                boot_id,
                last_shutdown_clean: true,
                observed_at_ms,
            }))),
            StoreCommand::CreateEnrollmentInvite {
                creation,
                creator,
                owner_addr,
                reply,
            } => drop(
                reply.send(
                    self.repository
                        .create_enrollment_invite(
                            creation.space_id,
                            creator,
                            owner_addr,
                            creation.validity,
                            &creation.entropy,
                        )
                        .map_err(Into::into),
                ),
            ),
            StoreCommand::CancelEnrollmentInvite {
                invitation_id,
                reply,
            } => drop(
                reply.send(
                    self.repository
                        .cancel_invitation(invitation_id)
                        .map_err(Into::into),
                ),
            ),
            StoreCommand::RedeemEnrollment { authorized, reply } => drop(
                reply.send(
                    self.repository
                        .redeem_enrollment(&authorized)
                        .map_err(Into::into),
                ),
            ),
            StoreCommand::PersistEnrollment { request, reply } => {
                drop(reply.send(self.persist_enrollment(*request)));
            }
            StoreCommand::AddressRecord {
                space_id,
                endpoint_id,
                reply,
            } => drop(
                reply.send(
                    self.repository
                        .address_record(space_id, endpoint_id)
                        .map_err(Into::into),
                ),
            ),
            StoreCommand::AdvanceOwnedSpace {
                update,
                local_endpoint_id,
                reply,
            } => drop(reply.send(self.advance_owned_space(&update, local_endpoint_id))),
            StoreCommand::PublishAddress {
                publisher,
                local_endpoint_id,
                now_ms,
                force_advance,
                reply,
            } => drop(reply.send(self.publish_address(
                &publisher,
                local_endpoint_id,
                now_ms,
                force_advance,
            ))),
            StoreCommand::PublishRelayAdvertisements {
                publisher,
                local_endpoint_id,
                issued_at_ms,
                expires_at_ms,
                reply,
            } => drop(reply.send(self.publish_relay_advertisements(
                &publisher,
                local_endpoint_id,
                issued_at_ms,
                expires_at_ms,
            ))),
            StoreCommand::ReconcileRelayActivity {
                local_endpoint_id,
                active_spaces,
                reply,
            } => drop(reply.send(self.reconcile_relay_activity(local_endpoint_id, &active_spaces))),
            StoreCommand::LoadControlLookup {
                local_endpoint_id,
                now_ms,
                reply,
            } => drop(reply.send(crate::control_sync::load_lookup(
                &mut self.repository,
                local_endpoint_id,
                now_ms,
            ))),
            StoreCommand::LoadRelayMap {
                local_endpoint_id,
                now_ms,
                reply,
            } => drop(reply.send(self.load_relay_map(local_endpoint_id, now_ms))),
            StoreCommand::RecordRelayObservations {
                observations,
                reply,
            } => drop(reply.send(self.record_relay_observations(&observations))),
            StoreCommand::RelayConfiguration { reply } => self.reply_relay_configuration(reply),
            StoreCommand::SetRelayConfiguration {
                configuration,
                reply,
            } => self.reply_set_relay_configuration(&configuration, reply),
            StoreCommand::RelayAuthorizations {
                local_endpoint_id,
                reply,
            } => self.reply_relay_authorizations(local_endpoint_id, reply),
            StoreCommand::PrepareControlRound { input, reply } => {
                drop(reply.send(crate::control_sync::prepare_round(&self.repository, &input)));
            }
            StoreCommand::RespondControl { input, reply } => {
                local_control::respond(&mut self.repository, &input, reply);
            }
            StoreCommand::AuthorizeControl { input, reply } => {
                local_control::authorize(&self.repository, &input, reply);
            }
            StoreCommand::ApplyControlResponse { input, reply } => drop(reply.send(
                crate::control_sync::apply_response(&mut self.repository, &input),
            )),
            StoreCommand::AuthorizeEcho {
                local_endpoint_id,
                peer_endpoint_id,
                reply,
            } => drop(reply.send(crate::services::echo::authorize(
                &self.repository,
                local_endpoint_id,
                peer_endpoint_id,
            ))),
            StoreCommand::AdvanceRevision(reply) => {
                drop(reply.send(self.repository.advance_revision().map_err(Into::into)));
            }
            StoreCommand::Stop(reply) => {
                let _unsent = reply.send(());
                return true;
            }
        }
        false
    }
}
