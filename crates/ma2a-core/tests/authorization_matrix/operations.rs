use ma2a_core::{AuthorizationDenied, RemoteOperation, authorize_endpoint};
use proptest::{prelude::*, test_runner::TestCaseError};

use super::fixtures::{
    AccessSpec, CapabilityGrants, Endpoints, PolicyGrants, Revocation, TestResult, endpoints,
    request, spec, view,
};

#[derive(Clone, Copy, Debug)]
struct OperationCase {
    name: &'static str,
    operation: RemoteOperation,
    complete: AccessSpec,
    denied: AccessSpec,
}

#[derive(Clone, Copy)]
struct SpaceOutcome {
    complete: bool,
    authority: u8,
}

const MEMBERSHIP_ONLY: AccessSpec = AccessSpec::new(
    CapabilityGrants::NONE,
    CapabilityGrants::NONE,
    PolicyGrants::NONE,
);
const ECHO_COMPLETE: AccessSpec = AccessSpec::new(
    CapabilityGrants::ECHO,
    CapabilityGrants::ECHO,
    PolicyGrants::ECHO,
);
const ECHO_POLICY_DENIED: AccessSpec = AccessSpec::new(
    CapabilityGrants::ECHO,
    CapabilityGrants::ECHO,
    PolicyGrants::NONE,
);
const RELAY_COMPLETE: AccessSpec = AccessSpec::new(
    CapabilityGrants::RELAY,
    CapabilityGrants::NONE,
    PolicyGrants::RELAY,
);
const RELAY_POLICY_DENIED: AccessSpec = AccessSpec::new(
    CapabilityGrants::RELAY,
    CapabilityGrants::NONE,
    PolicyGrants::NONE,
);

const OPERATION_CASES: [OperationCase; 5] = [
    OperationCase {
        name: "echo call",
        operation: RemoteOperation::ECHO_CALL,
        complete: ECHO_COMPLETE,
        denied: ECHO_POLICY_DENIED,
    },
    OperationCase {
        name: "control sync",
        operation: RemoteOperation::CONTROL_SYNC,
        complete: MEMBERSHIP_ONLY,
        denied: MEMBERSHIP_ONLY,
    },
    OperationCase {
        name: "metadata read",
        operation: RemoteOperation::METADATA_READ,
        complete: MEMBERSHIP_ONLY,
        denied: MEMBERSHIP_ONLY,
    },
    OperationCase {
        name: "address record exchange",
        operation: RemoteOperation::ADDRESS_RECORD_EXCHANGE,
        complete: MEMBERSHIP_ONLY,
        denied: MEMBERSHIP_ONLY,
    },
    OperationCase {
        name: "relay advertisement",
        operation: RemoteOperation::RELAY_ADVERTISEMENT,
        complete: RELAY_COMPLETE,
        denied: RELAY_POLICY_DENIED,
    },
];

#[test]
fn every_supported_operation_distinguishes_zero_one_and_many_shared_spaces() -> TestResult {
    // Given
    let endpoints = endpoints();

    for (index, case) in OPERATION_CASES.into_iter().enumerate() {
        let complete = view(
            endpoints,
            spec(authority(index, 0), case.complete, Revocation::None),
        )?;
        let denied = view(
            endpoints,
            spec(authority(index, 1), case.denied, Revocation::Caller),
        )?;
        let request = request(endpoints, case.operation);

        // When
        let zero = authorize_endpoint(&request, &[]);
        let one = authorize_endpoint(&request, std::slice::from_ref(&complete));
        let many = authorize_endpoint(&request, &[denied.clone(), complete, denied]);

        // Then
        assert_eq!(
            zero.err(),
            Some(AuthorizationDenied::ACCESS_DENIED),
            "{}",
            case.name
        );
        assert!(one.is_ok(), "{}", case.name);
        assert!(many.is_ok(), "{}", case.name);
    }
    Ok(())
}

#[test]
fn complete_allow_in_one_space_wins_over_other_space_denial() -> TestResult {
    // Given
    let endpoints = endpoints();
    let denied = view(endpoints, spec(0x31, ECHO_POLICY_DENIED, Revocation::None))?;
    let allowed = view(endpoints, spec(0x32, ECHO_COMPLETE, Revocation::None))?;

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
    let caller_only = AccessSpec::new(
        CapabilityGrants::ECHO,
        CapabilityGrants::NONE,
        PolicyGrants::ECHO,
    );
    let target_only = AccessSpec::new(
        CapabilityGrants::NONE,
        CapabilityGrants::ECHO,
        PolicyGrants::ECHO,
    );
    let first = view(endpoints, spec(0x33, caller_only, Revocation::None))?;
    let second = view(endpoints, spec(0x34, target_only, Revocation::None))?;

    // When
    let decision = authorize_endpoint(
        &request(endpoints, RemoteOperation::ECHO_CALL),
        &[first, second],
    );

    // Then
    assert_eq!(decision.err(), Some(AuthorizationDenied::ACCESS_DENIED));
    Ok(())
}

#[test]
fn revocation_is_local_to_its_space_for_every_supported_operation() -> TestResult {
    // Given
    let endpoints = endpoints();

    for (index, case) in OPERATION_CASES.into_iter().enumerate() {
        let revoked = view(
            endpoints,
            spec(authority(index, 2), case.complete, Revocation::Caller),
        )?;
        let independent = view(
            endpoints,
            spec(authority(index, 3), case.complete, Revocation::None),
        )?;
        let request = request(endpoints, case.operation);

        // When
        let revoked_only = authorize_endpoint(&request, std::slice::from_ref(&revoked));
        let with_independent_allow = authorize_endpoint(&request, &[revoked, independent]);

        // Then
        assert_eq!(
            revoked_only.err(),
            Some(AuthorizationDenied::ACCESS_DENIED),
            "{}",
            case.name
        );
        assert!(with_independent_allow.is_ok(), "{}", case.name);
    }
    Ok(())
}

#[test]
fn revoked_local_target_cannot_be_authorized() -> TestResult {
    // Given
    let endpoints = endpoints();
    let revoked_target = view(endpoints, spec(0x3c, ECHO_COMPLETE, Revocation::Target))?;

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
fn denial_responses_do_not_reveal_space_cardinality_for_any_operation() -> TestResult {
    // Given
    let endpoints = endpoints();

    for (index, case) in OPERATION_CASES.into_iter().enumerate() {
        let one_denied = view(
            endpoints,
            spec(authority(index, 4), case.denied, Revocation::Caller),
        )?;
        let another_denied = view(
            endpoints,
            spec(authority(index, 5), case.denied, Revocation::Target),
        )?;
        let request = request(endpoints, case.operation);

        // When
        let zero = authorize_endpoint(&request, &[]).err();
        let one = authorize_endpoint(&request, std::slice::from_ref(&one_denied)).err();
        let many = authorize_endpoint(&request, &[one_denied, another_denied]).err();

        // Then
        assert_eq!(zero, one, "{}", case.name);
        assert_eq!(one, many, "{}", case.name);
        assert_eq!(
            many,
            Some(AuthorizationDenied::ACCESS_DENIED),
            "{}",
            case.name
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn authorization_matches_any_complete_space(
        case in prop::sample::select(OPERATION_CASES.to_vec()),
        first_complete in any::<bool>(),
        second_complete in any::<bool>(),
    ) {
        let endpoints = endpoints();
        let first = outcome_view(
            endpoints,
            case,
            SpaceOutcome {
                complete: first_complete,
                authority: 0x70,
            },
        )?;
        let second = outcome_view(
            endpoints,
            case,
            SpaceOutcome {
                complete: second_complete,
                authority: 0x71,
            },
        )?;
        let expected = first_complete || second_complete;

        let actual = authorize_endpoint(
            &request(endpoints, case.operation),
            &[first, second],
        );

        prop_assert_eq!(actual.is_ok(), expected, "{}", case.name);
    }
}

fn outcome_view(
    endpoints: Endpoints,
    case: OperationCase,
    outcome: SpaceOutcome,
) -> Result<ma2a_core::SpaceAuthorizationView, TestCaseError> {
    let access = if outcome.complete {
        case.complete
    } else {
        case.denied
    };
    let revocation = if outcome.complete {
        Revocation::None
    } else {
        Revocation::Caller
    };
    view(endpoints, spec(outcome.authority, access, revocation))
        .map_err(|error| TestCaseError::fail(error.to_string()))
}

fn authority(case_index: usize, variant: u8) -> u8 {
    u8::try_from(case_index)
        .map_or(0x40, |index| 0x40_u8.wrapping_add(index * 8))
        .wrapping_add(variant)
}
