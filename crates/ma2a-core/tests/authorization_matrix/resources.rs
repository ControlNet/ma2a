use ma2a_core::{
    AuthorizationDenied, AuthorizationEndpoints, AuthorizationRequest, AuthorizationResource,
    RemoteOperation, authorize_endpoint,
};

use super::fixtures::{
    AccessSpec, CapabilityGrants, PolicyGrants, Revocation, TestResult, endpoints, request, spec,
    view,
};

const SHARED_MEMBERSHIP: AccessSpec = AccessSpec::new(
    CapabilityGrants::NONE,
    CapabilityGrants::NONE,
    PolicyGrants::NONE,
);

#[test]
fn missing_shared_space_and_unknown_operation_deny_by_default() -> TestResult {
    // Given
    let endpoints = endpoints();
    let shared = view(endpoints, spec(0x61, SHARED_MEMBERSHIP, Revocation::None))?;

    // When
    let no_spaces = authorize_endpoint(&request(endpoints, RemoteOperation::ECHO_CALL), &[]);
    let unknown = authorize_endpoint(
        &request(endpoints, RemoteOperation::UNSUPPORTED),
        std::slice::from_ref(&shared),
    );
    let invalid_resource = AuthorizationRequest::new(
        AuthorizationEndpoints::new(endpoints.caller, endpoints.target),
        RemoteOperation::ECHO_CALL,
        Some(AuthorizationResource::control_space_cursor(
            shared.space_id(),
        )),
    );

    // Then
    assert_eq!(no_spaces.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    assert_eq!(unknown.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    assert_eq!(
        authorize_endpoint(&invalid_resource, &[shared]).err(),
        Some(AuthorizationDenied::ACCESS_DENIED)
    );
    Ok(())
}

#[test]
fn control_cursor_only_narrows_an_already_shared_space() -> TestResult {
    // Given
    let endpoints = endpoints();
    let shared = view(endpoints, spec(0x62, SHARED_MEMBERSHIP, Revocation::None))?;
    let unrelated = view(endpoints, spec(0x63, SHARED_MEMBERSHIP, Revocation::None))?;
    let request = AuthorizationRequest::new(
        AuthorizationEndpoints::new(endpoints.caller, endpoints.target),
        RemoteOperation::CONTROL_SYNC,
        Some(AuthorizationResource::control_space_cursor(
            unrelated.space_id(),
        )),
    );

    // When
    let decision = authorize_endpoint(&request, &[shared]);

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}

#[test]
fn unsupported_denials_do_not_reveal_space_cardinality() -> TestResult {
    // Given
    let endpoints = endpoints();
    let first = view(endpoints, spec(0x64, SHARED_MEMBERSHIP, Revocation::None))?;
    let second = view(endpoints, spec(0x65, SHARED_MEMBERSHIP, Revocation::None))?;
    let request = request(endpoints, RemoteOperation::UNSUPPORTED);

    // When
    let zero = authorize_endpoint(&request, &[]).err();
    let one = authorize_endpoint(&request, std::slice::from_ref(&first)).err();
    let many = authorize_endpoint(&request, &[first, second]).err();

    // Then
    assert_eq!(zero, one);
    assert_eq!(one, many);
    assert_eq!(many, Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}
