use ma2a_core::{AuthorizationDenied, RemoteOperation, authorize_endpoint};

use super::fixtures::{
    AccessSpec, CapabilityGrants, PolicyGrants, Revocation, TestResult, endpoints, request, spec,
    view,
};

const ECHO_POLICY_EXCLUDED: AccessSpec = AccessSpec::new(
    CapabilityGrants::ECHO,
    CapabilityGrants::ECHO,
    PolicyGrants::NONE,
);

#[test]
fn echo_is_denied_when_sole_shared_space_policy_excludes_echo() -> TestResult {
    // Given
    let endpoints = endpoints();
    let shared_space = view(
        endpoints,
        spec(0x30, ECHO_POLICY_EXCLUDED, Revocation::None),
    )?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::ECHO_CALL),
        &[shared_space],
    );

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}
