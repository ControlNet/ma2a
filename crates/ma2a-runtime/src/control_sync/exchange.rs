use ma2a_core::{
    AuthorizationEndpoints, AuthorizationRequest, AuthorizationResource, ControlCursorV1,
    ControlPageV1, ControlRequestV1, ControlResponseV1, RemoteOperation,
};
use ma2a_net::ControlRejection;
use ma2a_store::{ControlSpaceState, Repository};

use super::{
    ControlApplyOutcome, ControlAuthorizationInput, ControlExchangeInput, ControlFailure,
    ControlRespondOutcome, pages,
};

pub(crate) fn respond(
    repository: &mut Repository,
    input: &ControlExchangeInput,
) -> Result<ControlRespondOutcome, ControlFailure> {
    let mut shared =
        repository.control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)?;
    authorize_shared(&shared, input.local_endpoint_id, input.remote_endpoint_id)?;
    let request =
        ControlRequestV1::decode(&input.payload).map_err(|_| ControlRejection::Invalid)?;
    let authorizations = shared
        .iter()
        .map(ControlSpaceState::authorization)
        .collect::<Vec<_>>();
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
    shared =
        repository.control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)?;
    pages::validate_cursor_spaces(&shared, request.cursors())?;
    let artifact_budget = pages::response_artifact_budget(request.cursors().len())?;
    let pages = request
        .cursors()
        .iter()
        .filter_map(|cursor| {
            shared
                .iter()
                .find(|state| state.chain().space_id() == cursor.space_id())
                .map(|state| {
                    pages::pull_page(
                        state,
                        cursor,
                        pages::PullPageRequest {
                            now_ms: input.now_ms,
                            artifact_budget,
                        },
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let response = ControlResponseV1::new(pages)
        .and_then(|response| response.encode())
        .map_err(|_| ControlRejection::Invalid)?;
    Ok(ControlRespondOutcome {
        response,
        revision: repository.revision()?,
        changes,
    })
}

pub(crate) fn authorize(
    repository: &Repository,
    input: &ControlAuthorizationInput,
) -> Result<(), ControlFailure> {
    let shared =
        repository.control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)?;
    authorize_shared(&shared, input.local_endpoint_id, input.remote_endpoint_id).map_err(Into::into)
}

fn authorize_shared(
    shared: &[ControlSpaceState],
    local_endpoint_id: ma2a_core::EndpointId,
    remote_endpoint_id: ma2a_core::EndpointId,
) -> Result<(), ControlRejection> {
    if shared.is_empty() {
        return Err(ControlRejection::Unauthorized);
    }
    let authorizations = shared
        .iter()
        .map(ControlSpaceState::authorization)
        .collect::<Vec<_>>();
    crate::authz::authorize_remote(
        &AuthorizationRequest::new(
            AuthorizationEndpoints::new(remote_endpoint_id, local_endpoint_id),
            RemoteOperation::CONTROL_SYNC,
            None,
        ),
        &authorizations,
    )
    .map_err(|_| ControlRejection::Unauthorized)?;
    Ok(())
}

pub(crate) fn apply_response(
    repository: &mut Repository,
    input: &ControlExchangeInput,
) -> Result<ControlApplyOutcome, ControlFailure> {
    let shared =
        repository.control_spaces_between(input.local_endpoint_id, input.remote_endpoint_id)?;
    if shared.is_empty() {
        return Err(ControlRejection::Unauthorized.into());
    }
    let response =
        ControlResponseV1::decode(&input.payload).map_err(|_| ControlRejection::Invalid)?;
    pages::validate_page_spaces(&shared, response.pages())
        .map_err(|_| ControlRejection::Invalid)?;
    let changes = pages::apply_pages(
        repository,
        pages::PageApplication::new(&shared, response.pages(), input.now_ms),
    )?;
    Ok(ControlApplyOutcome {
        revision: repository.revision()?,
        changes,
    })
}
