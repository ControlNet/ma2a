use ma2a_core::{EndpointId, RequestId, SpaceId};

use super::{Command, CommandKind, PrivateRelayConfiguration};

impl Command {
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
            CommandKind::SpaceShow(space_id) => Some(space_id),
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
