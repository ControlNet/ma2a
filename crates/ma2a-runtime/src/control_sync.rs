mod pages;
mod scheduler;
mod scope;

use std::collections::{BTreeMap, BTreeSet};

use ma2a_core::{
    AuthorizationEndpoints, AuthorizationRequest, AuthorizationResource, ControlCursorV1,
    ControlPageV1, ControlRequestV1, ControlResponseV1, EndpointId, RemoteOperation,
    SpaceAuthorizationView,
};
use ma2a_net::{
    AddressRecordTarget, AddressRecordValidationError, AddressRecordValidator, ControlRejection,
    SpaceAddressLookup, ValidatedAddressRecord,
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

pub(crate) fn prepare_round(
    repository: &Repository,
    input: &ControlRoundRequest,
) -> Result<Vec<PreparedControlPeer>, RuntimeError> {
    let spaces = repository.control_spaces_for(input.local_endpoint_id)?;
    let mut by_peer = BTreeMap::<EndpointId, (Vec<ControlCursorV1>, Vec<ControlPageV1>)>::new();
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
            let entry = by_peer.entry(peer).or_default();
            entry.0.push(
                state
                    .cursor()
                    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?,
            );
            entry.1.push(pages::push_page(state, input.now_ms)?);
        }
    }
    by_peer
        .into_iter()
        .map(|(peer, (cursors, pages))| {
            let request = ControlRequestV1::new(cursors)
                .and_then(|request| request.with_push_pages(pages))
                .and_then(|request| request.encode())
                .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
            Ok(PreparedControlPeer { peer, request })
        })
        .collect()
}

pub(crate) fn respond(
    repository: &mut Repository,
    input: &ControlExchangeInput,
) -> Result<ControlRespondOutcome, ControlRejection> {
    let mut shared = repository
        .control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)
        .map_err(|_| ControlRejection::Unavailable)?;
    if shared.is_empty() {
        return Err(ControlRejection::Unauthorized);
    }
    let authorizations = shared
        .iter()
        .map(ControlSpaceState::authorization)
        .collect::<Vec<_>>();
    crate::authz::authorize_remote(
        &AuthorizationRequest::new(
            AuthorizationEndpoints::new(input.remote_endpoint_id, input.local_endpoint_id),
            RemoteOperation::CONTROL_SYNC,
            None,
        ),
        &authorizations,
    )
    .map_err(|_| ControlRejection::Unauthorized)?;
    let request =
        ControlRequestV1::decode(&input.payload).map_err(|_| ControlRejection::Invalid)?;
    for space_id in request
        .cursors()
        .iter()
        .map(ControlCursorV1::space_id)
        .chain(request.push_pages().iter().map(ControlPageV1::space_id))
    {
        crate::authz::authorize_remote(
            &AuthorizationRequest::new(
                AuthorizationEndpoints::new(input.remote_endpoint_id, input.local_endpoint_id),
                RemoteOperation::CONTROL_SYNC,
                Some(AuthorizationResource::control_space_cursor(space_id)),
            ),
            &authorizations,
        )
        .map_err(|_| ControlRejection::Unauthorized)?;
    }
    pages::validate_page_spaces(&shared, request.push_pages())?;
    let changes = pages::apply_pages(
        repository,
        pages::PageApplication::new(&shared, request.push_pages(), input.now_ms),
    )?;
    shared = repository
        .control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)
        .map_err(|_| ControlRejection::Unavailable)?;
    pages::validate_cursor_spaces(&shared, request.cursors())?;
    let pages = request
        .cursors()
        .iter()
        .filter_map(|cursor| {
            shared
                .iter()
                .find(|state| state.chain().space_id() == cursor.space_id())
                .map(|state| pages::pull_page(state, cursor, input.now_ms))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let response = ControlResponseV1::new(pages)
        .and_then(|response| response.encode())
        .map_err(|_| ControlRejection::Invalid)?;
    let lookup = load_lookup(repository, input.local_endpoint_id, input.now_ms)
        .map_err(|_| ControlRejection::Unavailable)?;
    Ok(ControlRespondOutcome {
        response,
        revision: repository
            .revision()
            .map_err(|_| ControlRejection::Unavailable)?,
        memberships: repository
            .memberships_for(input.local_endpoint_id)
            .map_err(|_| ControlRejection::Unavailable)?,
        lookup,
        changes,
    })
}

pub(crate) fn apply_response(
    repository: &mut Repository,
    input: &ControlExchangeInput,
) -> Result<ControlApplyOutcome, RuntimeError> {
    let shared =
        repository.control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)?;
    if shared.is_empty() {
        return Err(RuntimeError::new(RuntimeErrorKind::Control));
    }
    let response = ControlResponseV1::decode(&input.payload)
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    pages::validate_page_spaces(&shared, response.pages())
        .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    let changes = pages::apply_pages(
        repository,
        pages::PageApplication::new(&shared, response.pages(), input.now_ms),
    )
    .map_err(|_| RuntimeError::new(RuntimeErrorKind::Control))?;
    let lookup = load_lookup(repository, input.local_endpoint_id, input.now_ms)?;
    Ok(ControlApplyOutcome {
        revision: repository.revision()?,
        memberships: repository.memberships_for(input.local_endpoint_id)?,
        lookup,
        changes,
    })
}
