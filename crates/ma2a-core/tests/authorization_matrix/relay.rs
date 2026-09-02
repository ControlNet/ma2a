use ma2a_core::{AuthorizationDenied, RemoteOperation, authorize_endpoint};

use super::fixtures::{
    AccessSpec, CapabilityGrants, PolicyGrants, Revocation, TestResult, endpoints, request, spec,
    view,
};

const RELAY_ALLOWED: AccessSpec = AccessSpec::new(
    CapabilityGrants::RELAY,
    CapabilityGrants::NONE,
    PolicyGrants::RELAY,
);
const MEMBER_DENIED: AccessSpec = AccessSpec::new(
    CapabilityGrants::NONE,
    CapabilityGrants::NONE,
    PolicyGrants::RELAY,
);
const POLICY_DENIED: AccessSpec = AccessSpec::new(
    CapabilityGrants::RELAY,
    CapabilityGrants::NONE,
    PolicyGrants::NONE,
);

#[test]
fn relay_provider_member_and_policy_grant_allows_advertisement() -> TestResult {
    // Given
    let endpoints = endpoints();
    let allowed = view(endpoints, spec(0x51, RELAY_ALLOWED, Revocation::None))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::RELAY_ADVERTISEMENT),
        &[allowed],
    );

    // Then
    assert!(decision.is_ok());
    Ok(())
}

#[test]
fn relay_advertisement_does_not_require_member_capability() -> TestResult {
    // Given
    let endpoints = endpoints();
    let allowed = view(endpoints, spec(0x52, MEMBER_DENIED, Revocation::None))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::RELAY_ADVERTISEMENT),
        &[allowed],
    );

    // Then
    assert!(decision.is_ok());
    Ok(())
}

#[test]
fn relay_advertisement_denies_policy_exclusion() -> TestResult {
    assert_relay_denied(0x53, POLICY_DENIED, Revocation::None)
}

#[test]
fn relay_advertisement_denies_revoked_provider() -> TestResult {
    assert_relay_denied(0x54, RELAY_ALLOWED, Revocation::Caller)
}

#[test]
fn relay_revocation_in_one_space_does_not_cancel_independent_allow() -> TestResult {
    // Given
    let endpoints = endpoints();
    let revoked = view(endpoints, spec(0x55, RELAY_ALLOWED, Revocation::Caller))?;
    let independent = view(endpoints, spec(0x56, RELAY_ALLOWED, Revocation::None))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::RELAY_ADVERTISEMENT),
        &[revoked, independent],
    );

    // Then
    assert!(decision.is_ok());
    Ok(())
}

#[test]
fn relay_member_and_policy_privileges_never_compose_across_spaces() -> TestResult {
    // Given
    let endpoints = endpoints();
    let member_only = view(endpoints, spec(0x57, POLICY_DENIED, Revocation::None))?;
    let policy_only = view(endpoints, spec(0x58, MEMBER_DENIED, Revocation::Caller))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::RELAY_ADVERTISEMENT),
        &[member_only, policy_only],
    );

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}

fn assert_relay_denied(authority: u8, access: AccessSpec, revocation: Revocation) -> TestResult {
    // Given
    let endpoints = endpoints();
    let denied = view(endpoints, spec(authority, access, revocation))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::RELAY_ADVERTISEMENT),
        &[denied],
    );

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}
