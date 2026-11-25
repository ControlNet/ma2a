mod dial_queue;
mod exchange;
mod failure;
pub(crate) use failure::ControlFailure;
mod pages;
#[cfg(test)]
mod relay_export_tests;
#[cfg(test)]
mod respond_tests;
#[cfg(test)]
mod round_tests;
mod scheduler;
mod scope;

use std::collections::BTreeMap;

use ma2a_core::{
    ControlRequestV1, EndpointId, MAX_CONTROL_SPACES_PER_REQUEST, SpaceAuthorizationView, SpaceId,
};
use ma2a_net::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator, SpaceAddressLookup,
    ValidatedAddressRecord,
};
use ma2a_store::{ControlSpaceState, Repository};

use crate::error::{RuntimeError, RuntimeErrorKind};

#[derive(Debug)]
pub(crate) struct ControlLookupState {
    pub(crate) authorizations: Vec<SpaceAuthorizationView>,
    pub(crate) addresses: Vec<ValidatedAddressRecord>,
}

#[derive(Debug)]
pub(crate) struct PreparedControlPeer {
    pub(crate) peer: EndpointId,
    pub(crate) request: Vec<u8>,
    /// The Spaces the request asks about; a response may answer only these.
    pub(crate) spaces: Vec<SpaceId>,
}

#[derive(Debug)]
pub(crate) struct ControlExchangeInput {
    pub(crate) local_endpoint_id: EndpointId,
    pub(crate) remote_endpoint_id: EndpointId,
    pub(crate) payload: Vec<u8>,
    pub(crate) now_ms: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ControlAuthorizationInput {
    pub(crate) local_endpoint_id: EndpointId,
    pub(crate) remote_endpoint_id: EndpointId,
}

#[derive(Debug)]
pub(crate) struct ControlApplyOutcome {
    pub(crate) revision: u64,
    pub(crate) changes: ControlChanges,
}

#[derive(Debug)]
pub(crate) struct ControlRespondOutcome {
    pub(crate) response: Vec<u8>,
    pub(crate) revision: u64,
    pub(crate) changes: ControlChanges,
}

pub(crate) use exchange::{apply_response, authorize, respond};
pub(crate) use pages::ControlChanges;
pub(crate) use scheduler::ControlRoundRunner;
pub(crate) use scope::{
    ControlRoundOutcome, ControlRoundRequest, ControlRoundScope, ControlRoundTrigger,
};

pub(crate) fn install_lookup(
    lookup: &SpaceAddressLookup,
    state: ControlLookupState,
) -> Result<(), RuntimeError> {
    lookup
        .replace_authorizations(state.authorizations)
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    for address in state.addresses {
        lookup
            .cache(address)
            .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    }
    Ok(())
}

pub(crate) fn load_lookup(
    repository: &mut Repository,
    local_endpoint_id: EndpointId,
    now_ms: u64,
) -> Result<ControlLookupState, RuntimeError> {
    let spaces = repository.control_spaces_for(local_endpoint_id)?;
    let authorizations = spaces
        .iter()
        .map(ControlSpaceState::authorization)
        .collect::<Vec<_>>();
    let mut addresses = Vec::new();
    for state in &spaces {
        let authorization = state.authorization();
        for record in state.address_records() {
            if !authorization.contains_member(record.endpoint_id()) {
                continue;
            }
            let Ok(issued_at_ms) = u64::try_from(record.issued_at_ms()) else {
                continue;
            };
            let Ok(expires_at_ms) = u64::try_from(record.expires_at_ms()) else {
                continue;
            };
            if issued_at_ms > now_ms || expires_at_ms <= now_ms {
                continue;
            }
            let target = AddressRecordTarget::new(record.space_id(), record.endpoint_id());
            match AddressRecordValidator::validate_and_store(
                repository,
                record.signed_record(),
                target.validation(&authorization, now_ms),
            ) {
                Ok(validated) => addresses.push(validated),
                // A record an earlier version accepted, such as one carrying a relay URL that
                // names a user, no longer decodes; it is left out like an expired one.
                Err(
                    AddressRecordValidationError::FutureRecord
                    | AddressRecordValidationError::ExpiredRecord
                    | AddressRecordValidationError::InvalidEncoding,
                ) => {}
                Err(_) => return Err(RuntimeError::new(RuntimeErrorKind::Control)),
            }
        }
    }
    Ok(ControlLookupState {
        authorizations,
        addresses,
    })
}

/// Spaces sent in one request: one fewer than the wire allows, so each response page's share of
/// the batch still holds the largest signed artifact, a full address record, and no valid record
/// is held back on every round by the size of its page's share.
pub(crate) const SPACES_PER_REQUEST: usize = MAX_CONTROL_SPACES_PER_REQUEST - 1;

/// Prepares every request of one round.
///
/// One request carries at most [`SPACES_PER_REQUEST`] Spaces, so a peer sharing more Spaces
/// receives several. The queue takes every peer's first request before any peer's
/// second, so a peer with many Spaces never delays the others.
pub(crate) fn prepare_round(
    repository: &Repository,
    input: &ControlRoundRequest,
) -> Result<Vec<PreparedControlPeer>, RuntimeError> {
    let spaces = repository.control_spaces_for(input.local_endpoint_id)?;
    let mut by_peer = BTreeMap::<EndpointId, Vec<&ControlSpaceState>>::new();
    for state in &spaces {
        let authorization = state.authorization();
        let fresh_targets = state
            .address_records()
            .iter()
            .filter(|record| {
                u64::try_from(record.issued_at_ms()).is_ok_and(|issued| issued <= input.now_ms)
                    && u64::try_from(record.expires_at_ms())
                        .is_ok_and(|expires| expires > input.now_ms)
                    && authorization.contains_member(record.endpoint_id())
            })
            .map(ma2a_store::PersistedAddressRecord::endpoint_id)
            .collect::<Vec<_>>();
        for peer in input.select(&fresh_targets) {
            by_peer.entry(peer).or_default().push(state);
        }
    }
    let mut queues = by_peer
        .into_iter()
        .map(|(peer, shared)| {
            shared
                .chunks(SPACES_PER_REQUEST)
                .map(|chunk| peer_request(peer, chunk, input))
                .collect::<Result<std::collections::VecDeque<_>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut round = Vec::new();
    while queues.iter().any(|queue| !queue.is_empty()) {
        round.extend(
            queues
                .iter_mut()
                .filter_map(std::collections::VecDeque::pop_front),
        );
    }
    Ok(round)
}

/// Builds one request to `peer` for at most one frame's worth of shared Spaces.
fn peer_request(
    peer: EndpointId,
    shared: &[&ControlSpaceState],
    input: &ControlRoundRequest,
) -> Result<PreparedControlPeer, RuntimeError> {
    let cursors = shared
        .iter()
        .map(|state| state.cursor())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    let artifact_budget = pages::request_artifact_budget(&cursors, shared.len())
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    let push_pages = shared
        .iter()
        .map(|state| {
            pages::push_page(
                state,
                pages::PushPageRequest {
                    local_endpoint_id: input.local_endpoint_id,
                    now_ms: input.now_ms,
                    artifact_budget,
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let request = ControlRequestV1::new(cursors)
        .and_then(|request| request.with_push_pages(push_pages))
        .and_then(|request| request.encode())
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    let spaces = shared
        .iter()
        .map(|state| state.chain().space_id())
        .collect();
    Ok(PreparedControlPeer {
        peer,
        request,
        spaces,
    })
}
