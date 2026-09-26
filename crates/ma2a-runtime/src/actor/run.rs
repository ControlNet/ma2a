use super::{Actor, Command, ShutdownAck, echo, relay_server};
use crate::{enrollment::EnrollmentError, error::RuntimeError, state::RuntimeEvent};

impl Actor {
    #[expect(clippy::too_many_lines, reason = "actor command ordering is explicit")]
    pub(crate) async fn run(mut self) -> Result<ShutdownAck, RuntimeError> {
        let _receiver_count = self.events.send(RuntimeEvent::ready(self.state.revision));
        self.schedule_control_round(crate::control_sync::ControlRoundTrigger::Startup, None);
        let control_period = crate::control_actor::control_period(self.state.endpoint_id);
        let mut periodic =
            tokio::time::interval_at(tokio::time::Instant::now() + control_period, control_period);
        loop {
            if self.maintenance.relay_configuration
                == Some(relay_server::RelayCompletion::ShutdownFailed)
            {
                return Err(RuntimeError::new(crate::error::RuntimeErrorKind::Shutdown));
            }
            tokio::select! {
                biased;
                () = self.cancellation.cancelled() => return self.finish(false).await,
                command = self.commands.recv() => match command {
                    Some(Command::Status(reply)) => {
                        let _unsent = reply.send(self.state.clone());
                    }
                    Some(Command::SpaceDetails(id, reply)) => self.handle_space_details(id, reply).await,
                    Some(Command::SnapshotStamp(reply)) => self.handle_snapshot_stamp(reply).await,
                    Some(Command::Snapshot(reply)) => self.handle_snapshot(reply).await,
                    Some(Command::ObserveMemberships { memberships, reply }) => {
                        let _unsent = reply.send(self.observe_memberships(memberships).await);
                    }
                    Some(Command::CreateOwnedSpace { name, reply }) => {
                        let _unsent = reply.send(self.create_owned_space(name).await);
                    }
                    Some(Command::RevokeOwnedSpaceMember { space_id, endpoint_id, reply }) => {
                        let _unsent = reply.send(self.revoke_owned_space_member(space_id, endpoint_id).await);
                    }
                    Some(Command::LeaveSpace { space_id, request_id, reply }) => {
                        let _unsent = reply.send(self.leave_space(space_id, request_id).await);
                    }
                    Some(Command::CreateEnrollmentInvite { creation, reply }) => {
                        let _unsent = reply.send(self.create_enrollment_invite(creation).await);
                    }
                    Some(Command::RedeemEnrollment { attempt, reply }) => {
                        let _unsent = reply.send(self.redeem_enrollment(*attempt).await);
                    }
                    Some(Command::CancelEnrollmentInvite { invitation_id, reply }) => {
                        let result = self.store.cancel_enrollment_invite(invitation_id).await
                            .map(|revision| { self.state.revision = revision; })
                            .map_err(|_| EnrollmentError::internal());
                        let _unsent = reply.send(result);
                    }
                    Some(Command::AdoptRevision { revision, reply }) => {
                        self.state.revision = self.state.revision.max(revision);
                        let _unsent = reply.send(self.state.revision);
                    }
                    Some(Command::ControlSyncStatus { peer, reply }) => {
                        let _unsent = reply.send(self.synchronized_control_peers.contains(&peer));
                    }
                    Some(Command::SyncControl { peer, reply }) => {
                        let scope = peer.map_or_else(
                            crate::control_sync::ControlRoundScope::all,
                            crate::control_sync::ControlRoundScope::peer,
                        );
                        self.schedule_control_round(
                            crate::control_sync::ControlRoundTrigger::Explicit(scope),
                            Some(reply),
                        );
                    }
                    Some(Command::AdvanceOwnedSpace { update, reply }) => {
                        let result = match self
                            .store
                            .advance_owned_space(update, self.state.endpoint_id)
                            .await
                        {
                            Ok((revision, memberships)) => {
                                self.state.revision = revision;
                                self.state.memberships = memberships;
                                self.endpoint
                                    .set_control_enabled(!self.state.memberships.is_empty());
                                match self.refresh_control_lookup().await {
                                    Ok(()) => match self.refresh_relay_candidates().await {
                                        Ok(_) => {
                                            self.schedule_control_round(
                                                crate::control_sync::ControlRoundTrigger::ManifestAdvanced,
                                                None,
                                            );
                                            Ok(revision)
                                        }
                                        Err(error) => Err(error),
                                    },
                                    Err(error) => Err(error),
                                }
                            }
                            Err(error) => Err(error),
                        };
                        let _unsent = reply.send(result);
                    }
                    Some(Command::PublishAddress(reply)) => {
                        let result = self.publish_local_address().await;
                        let _unsent = reply.send(result);
                    }
                    Some(Command::PublishRelayAdvertisements { config, expires_at_ms, reply }) => {
                        let result = self.publish_local_relay(config, expires_at_ms).await;
                        let _unsent = reply.send(result);
                    }
                    Some(Command::RelayConfiguration { reply }) => {
                        let _unsent = reply.send(self.store.relay_configuration().await);
                    }
                    Some(Command::RelayStatus { reply }) => {
                        let _unsent = reply.send(self.relay_runtime_status().await);
                    }
                    Some(Command::SetRelayConfiguration { configuration, mode, reply }) => {
                        let _unsent = reply.send(self.set_relay_configuration(configuration, mode).await);
                    }
                    Some(Command::Echo { request_id, target, payload, reply }) => {
                        self.spawn_outbound_echo(
                            echo::OutboundEchoRequest { request_id, target, payload },
                            reply,
                        );
                    }
                    Some(Command::Shutdown(reply)) => {
                        let result = self.finish(true).await;
                        if let Ok(ack) = result {
                            let _unsent = reply.send(ack);
                            return Ok(ack);
                        }
                        return result;
                    }
                    None => return self.finish(false).await,
                },
                call = self.enrollment_calls.recv() => if let Some(call) = call {
                    self.handle_enrollment_call(call).await?;
                },
                call = self.control_calls.recv() => if let Some(call) = call {
                    self.handle_control_call(call).await;
                },
                joined = self.control_tasks.join_next(), if !self.control_tasks.is_empty() => {
                    if let Some(Ok(completion)) = joined {
                        self.finish_control_call(completion).await;
                    }
                },
                call = self.echo_calls.recv() => if let Some(call) = call {
                    self.handle_echo_call(call).await;
                },
                joined = self.echo_tasks.join_next(), if !self.echo_tasks.is_empty() => {
                    if let Some(Ok(completion)) = joined {
                        self.finish_echo(completion).await;
                    }
                },
                observation = self.relay_observations.recv() => if let Some(observation) = observation {
                    let observed = self.observe_iroh_relay(observation).await;
                    Self::absorb_background("relay observation", observed)?;
                },
                joined = self.control_rounds.join_next(), if !self.control_rounds.is_empty() => {
                    if let Some(result) = joined {
                        self.finish_control_round(result).await;
                    }
                },
                _ = periodic.tick() => {
                    let configured = self.reconcile_relay_configuration().await;
                    Self::absorb_background("relay server configuration", configured)?;
                    let membership = self.reconcile_membership_completion().await;
                    Self::absorb_background("membership completion", membership)?;
                    let enrolled = self.reconcile_enrollment().await;
                    Self::absorb_background("enrollment completion", enrolled)?;
                    let refreshed = self.refresh_relay_candidates().await.map(|_changed| ());
                    Self::absorb_background("relay candidate refresh", refreshed)?;
                    let observed = self.reconcile_pending_iroh_observation().await;
                    Self::absorb_background("relay observation retry", observed)?;
                    let published = self.refresh_local_control_publications().await;
                    Self::absorb_background("control publication refresh", published)?;
                    self.schedule_control_round(
                        crate::control_sync::ControlRoundTrigger::Periodic, None,
                    );
                },
            }
        }
    }
}
