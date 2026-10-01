use ma2a_core::{EndpointId, RequestId, SpaceId};

use super::{Command, CommandKind, PrivateRelayConfiguration};

impl Command {
    /// Returns the request, target, and family of a stream-plane upgrade.
    pub const fn stream_open(&self) -> Option<(RequestId, EndpointId, ma2a_core::ServiceKind)> {
        match self.kind {
            CommandKind::StreamOpen(id, target, service) => Some((id, target, service)),
            _ => None,
        }
    }

    pub(crate) const fn process_command(&self) -> Option<&super::super::ProcessCommand> {
        match &self.kind {
            CommandKind::Process(command) => Some(command),
            _ => None,
        }
    }

    pub(crate) const fn hosted_command(&self) -> Option<&super::super::HostedCommand> {
        match &self.kind {
            CommandKind::Hosted(command) => Some(command),
            _ => None,
        }
    }

    /// Whether this command upgrades its connection into a hosted service's link or call.
    pub const fn is_hosted_upgrade(&self) -> bool {
        matches!(
            self.kind,
            CommandKind::Hosted(
                super::super::HostedCommand::Attach(_) | super::super::HostedCommand::Accept(..)
            )
        )
    }

    pub(crate) const fn agent_command(&self) -> Option<&super::super::AgentCommand> {
        match &self.kind {
            CommandKind::Agent(command) => Some(command),
            _ => None,
        }
    }

    pub(crate) const fn space_grant(&self) -> Option<((SpaceId, EndpointId), [bool; 8])> {
        match self.kind {
            CommandKind::SpaceGrant(_, space, peer, grants) => Some(((space, peer), grants)),
            _ => None,
        }
    }

    pub(crate) const fn service_exposure(
        &self,
    ) -> Option<(SpaceId, &super::super::codec_services::ExposureSpec)> {
        match &self.kind {
            CommandKind::ServiceExpose(_, space, spec)
            | CommandKind::ServiceUnexpose(_, space, spec) => Some((*space, spec)),
            _ => None,
        }
    }

    pub(crate) const fn control_sync_peer(&self) -> Option<EndpointId> {
        match self.kind {
            CommandKind::ControlSyncStatus(peer) | CommandKind::ControlSyncTrigger(_, peer) => {
                Some(peer)
            }
            _ => None,
        }
    }

    pub(crate) fn space_create_name(&self) -> Option<&str> {
        match &self.kind {
            CommandKind::SpaceCreate(_, name) => Some(name.as_str()),
            _ => None,
        }
    }

    pub(crate) const fn space_show_id(&self) -> Option<SpaceId> {
        match self.kind {
            CommandKind::SpaceShow(space_id) | CommandKind::SpaceDetailsFetch(space_id) => {
                Some(space_id)
            }
            _ => None,
        }
    }

    pub(crate) fn space_redeem(&self) -> Option<(RequestId, &str)> {
        match &self.kind {
            CommandKind::SpaceRedeem(request_id, invitation) => {
                Some((*request_id, invitation.as_str()))
            }
            _ => None,
        }
    }

    pub(crate) fn space_invite(&self) -> Option<(SpaceId, u64, &str)> {
        match &self.kind {
            CommandKind::SpaceInvite(_, space_id, ttl_ms, output_path) => {
                Some((*space_id, *ttl_ms, output_path.as_str()))
            }
            _ => None,
        }
    }

    pub(crate) const fn space_revoke(&self) -> Option<(SpaceId, EndpointId)> {
        match self.kind {
            CommandKind::SpaceRevoke(_, space_id, endpoint_id) => Some((space_id, endpoint_id)),
            _ => None,
        }
    }

    pub(crate) const fn space_leave(&self) -> Option<SpaceId> {
        match self.kind {
            CommandKind::SpaceLeave(_, space_id) => Some(space_id),
            _ => None,
        }
    }

    pub(crate) const fn private_relay_configuration(&self) -> Option<&PrivateRelayConfiguration> {
        match &self.kind {
            CommandKind::PrivateRelayConfigure(_, configuration) => Some(configuration),
            _ => None,
        }
    }

    pub(crate) fn public_relay_url(&self) -> Option<&str> {
        match &self.kind {
            CommandKind::PublicRelayConfigure(_, url) => Some(url.as_str()),
            _ => None,
        }
    }

    pub(crate) fn echo_call(&self) -> Option<(RequestId, EndpointId, &str)> {
        match &self.kind {
            CommandKind::EchoCall(request_id, target, payload) => {
                Some((*request_id, *target, payload.as_str()))
            }
            _ => None,
        }
    }
}
