mod exchange;
mod pages;
mod scheduler;
mod scope;

use std::collections::{BTreeMap, BTreeSet};

use ma2a_core::{ControlRequestV1, EndpointId, SpaceAuthorizationView};
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
}

#[derive(Debug)]
pub(crate) struct ControlExchangeInput {
    pub(crate) local_endpoint_id: EndpointId,
    pub(crate) remote_endpoint_id: EndpointId,
    pub(crate) payload: Vec<u8>,
    pub(crate) now_ms: u64,
}

#[derive(Debug)]
pub(crate) struct ControlApplyOutcome {
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<ma2a_core::SpaceId>,
    pub(crate) lookup: ControlLookupState,
    pub(crate) changes: ControlChanges,
}

#[derive(Debug)]
pub(crate) struct ControlRespondOutcome {
    pub(crate) response: Vec<u8>,
    pub(crate) revision: u64,
    pub(crate) memberships: BTreeSet<ma2a_core::SpaceId>,
    pub(crate) lookup: ControlLookupState,
    pub(crate) changes: ControlChanges,
}

pub(crate) use exchange::{apply_response, respond};
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
                Err(
                    AddressRecordValidationError::FutureRecord
                    | AddressRecordValidationError::ExpiredRecord,
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

#[expect(
    clippy::indexing_slicing,
    reason = "indexes are created by enumerating this unchanged collection"
)]
pub(crate) fn prepare_round(
    repository: &Repository,
    input: &ControlRoundRequest,
) -> Result<Vec<PreparedControlPeer>, RuntimeError> {
    let spaces = repository.control_spaces_for(input.local_endpoint_id)?;
    let mut by_peer = BTreeMap::<EndpointId, Vec<usize>>::new();
    for (space_index, state) in spaces.iter().enumerate() {
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
            by_peer.entry(peer).or_default().push(space_index);
        }
    }
    by_peer
        .into_iter()
        .map(|(peer, space_indexes)| {
            let cursors = space_indexes
                .iter()
                .map(|index| spaces[*index].cursor())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
            let artifact_budget = pages::request_artifact_budget(&cursors, space_indexes.len())
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
            let push_pages = space_indexes
                .iter()
                .map(|index| {
                    pages::push_page(
                        &spaces[*index],
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
            Ok(PreparedControlPeer { peer, request })
        })
        .collect()
}
