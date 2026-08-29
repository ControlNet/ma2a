//! Endpoint-centric independent-Space authorization matrix.

use iroh_base::SecretKey;
use ma2a_core::{
    AuthorizationDenied, AuthorizationEndpoints, AuthorizationRequest, AuthorizationResource,
    EndpointId, MemberCapabilities, RemoteOperation, SpaceAuthoritySecret, SpaceAuthorizationView,
    SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceManifestLink,
    SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1, SpaceRevocationV1,
    authorize_endpoint,
};
use proptest::{prelude::*, test_runner::TestCaseError};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Copy)]
struct Endpoints {
    caller: EndpointId,
    target: EndpointId,
}

#[derive(Clone, Copy)]
struct SpaceSpec {
    authority: u8,
    caller_echo: bool,
    target_echo: bool,
    revocation: Revocation,
}

#[derive(Clone, Copy)]
enum Revocation {
    None,
    Caller,
    Target,
}

#[test]
fn complete_allow_in_one_space_wins_over_other_space_denial() -> TestResult {
    // Given
    let endpoints = endpoints();
    let denied = view(endpoints, spec(0x31, (true, false, Revocation::None)))?;
    let allowed = view(endpoints, spec(0x32, (true, true, Revocation::None)))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::ECHO_CALL),
        &[denied, allowed],
    );

    // Then
    assert!(decision.is_ok());
    Ok(())
}

#[test]
fn partial_privileges_never_compose_across_spaces() -> TestResult {
    // Given
    let endpoints = endpoints();
    let caller_only = view(endpoints, spec(0x33, (true, false, Revocation::None)))?;
    let target_only = view(endpoints, spec(0x34, (false, true, Revocation::None)))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::ECHO_CALL),
        &[caller_only, target_only],
    );

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}

#[test]
fn revocation_is_local_to_its_space() -> TestResult {
    // Given
    let endpoints = endpoints();
    let revoked = view(endpoints, spec(0x35, (true, true, Revocation::Caller)))?;
    let independent = view(endpoints, spec(0x36, (true, true, Revocation::None)))?;
    let request = request(endpoints, RemoteOperation::ECHO_CALL);

    // When
    let revoked_only = authorize_endpoint(&request, std::slice::from_ref(&revoked));
    let with_independent_allow = authorize_endpoint(&request, &[revoked, independent]);

    // Then
    assert_eq!(revoked_only.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    assert!(with_independent_allow.is_ok());
    Ok(())
}

#[test]
fn revoked_local_target_cannot_be_authorized() -> TestResult {
    // Given
    let endpoints = endpoints();
    let revoked_target = view(endpoints, spec(0x3c, (true, true, Revocation::Target)))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::ECHO_CALL),
        &[revoked_target],
    );

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}

#[test]
fn missing_shared_space_and_unknown_operation_deny_by_default() -> TestResult {
    // Given
    let endpoints = endpoints();
    let shared = view(endpoints, spec(0x37, (true, true, Revocation::None)))?;

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
    let shared = view(endpoints, spec(0x38, (true, true, Revocation::None)))?;
    let unrelated = view(endpoints, spec(0x39, (true, true, Revocation::None)))?;
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
fn all_denials_are_externally_equal() -> TestResult {
    // Given
    let endpoints = endpoints();
    let one_partial = view(endpoints, spec(0x3a, (true, false, Revocation::None)))?;
    let another_partial = view(endpoints, spec(0x3b, (false, true, Revocation::None)))?;
    let request = request(endpoints, RemoteOperation::ECHO_CALL);

    // When
    let zero = authorize_endpoint(&request, &[]).err();
    let one = authorize_endpoint(&request, std::slice::from_ref(&one_partial)).err();
    let many = authorize_endpoint(&request, &[one_partial, another_partial]).err();

    // Then
    assert_eq!(zero, one);
    assert_eq!(one, many);
    assert_eq!(many, Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn authorization_matches_any_complete_space(
        a_caller in any::<bool>(), a_target in any::<bool>(), a_revoked in any::<bool>(),
        b_caller in any::<bool>(), b_target in any::<bool>(), b_revoked in any::<bool>(),
    ) {
        let endpoints = endpoints();
        let first_revocation = if a_revoked { Revocation::Caller } else { Revocation::None };
        let second_revocation = if b_revoked { Revocation::Caller } else { Revocation::None };
        let first = view(endpoints, spec(0x41, (a_caller, a_target, first_revocation)))
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        let second = view(endpoints, spec(0x42, (b_caller, b_target, second_revocation)))
            .map_err(|error| TestCaseError::fail(error.to_string()))?;
        let expected = (a_caller && a_target && !a_revoked)
            || (b_caller && b_target && !b_revoked);

        let actual = authorize_endpoint(
            &request(endpoints, RemoteOperation::ECHO_CALL),
            &[first, second],
        );

        prop_assert_eq!(actual.is_ok(), expected);
    }
}

const fn spec(authority: u8, access: (bool, bool, Revocation)) -> SpaceSpec {
    SpaceSpec {
        authority,
        caller_echo: access.0,
        target_echo: access.1,
        revocation: access.2,
    }
}

fn endpoints() -> Endpoints {
    Endpoints {
        caller: SecretKey::from_bytes(&[0x21; 32]).public().into(),
        target: SecretKey::from_bytes(&[0x22; 32]).public().into(),
    }
}

const fn request(endpoints: Endpoints, operation: RemoteOperation) -> AuthorizationRequest {
    AuthorizationRequest::new(
        AuthorizationEndpoints::new(endpoints.caller, endpoints.target),
        operation,
        None,
    )
}

fn view(endpoints: Endpoints, spec: SpaceSpec) -> TestResult<SpaceAuthorizationView> {
    let authority = SpaceAuthoritySecret::from_bytes([spec.authority; 32]);
    let target = member(endpoints.target, "target", spec.target_echo)?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new(
            [spec.authority.wrapping_add(1); 32],
            1,
            authority.public_key(),
        )?,
        SpaceGenesisOwner::new(target.clone(), SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let caller = member(endpoints.caller, "caller", spec.caller_echo)?;
    let mut members = vec![caller.clone(), target.clone()];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(genesis.space_id(), 1, genesis.chain_hash()),
        2,
        SpaceManifestMembership::new(members, Vec::new()),
    )?
    .sign(&authority)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    chain.apply(&first)?;
    let revocation = match spec.revocation {
        Revocation::None => None,
        Revocation::Caller => Some((vec![target], endpoints.caller)),
        Revocation::Target => Some((vec![caller], endpoints.target)),
    };
    if let Some((members, revoked_endpoint)) = revocation {
        let revoked = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
            3,
            SpaceManifestMembership::new(members, vec![SpaceRevocationV1::new(revoked_endpoint)]),
        )?
        .sign(&authority)?;
        chain.apply(&revoked)?;
    }
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

fn member(endpoint_id: EndpointId, label: &str, echo: bool) -> TestResult<SpaceMemberV1> {
    Ok(SpaceMemberV1::new(
        endpoint_id,
        label.to_owned(),
        MemberCapabilities::new(echo, false),
    )?)
}
