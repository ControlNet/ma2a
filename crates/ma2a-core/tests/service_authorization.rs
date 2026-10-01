//! Single-complete-Space authorization for built-in remote services.

use iroh_base::SecretKey;
use ma2a_core::{
    AuthorizationEndpoints, Capability, EndpointId, MemberCapabilities, SpaceAuthoritySecret,
    SpaceAuthorizationView, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1,
    SpaceRevocationV1,
    service::{
        AgentId, ExportedRoot, ProxyHost, ProxyHostPattern, ProxyRule, RootAccess, RootName,
        ServiceAccess, ServiceAuthorizationRequest, ServiceName, SpaceExposureView,
        authorize_service,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn endpoint(seed: u8) -> EndpointId {
    SecretKey::from_bytes(&[seed; 32]).public().into()
}

const CALLER: u8 = 0x31;
const TARGET: u8 = 0x32;

struct SpaceFixture {
    authority: u8,
    caller: MemberCapabilities,
    revoke_caller: bool,
}

fn space(fixture: &SpaceFixture) -> TestResult<SpaceAuthorizationView> {
    let authority = SpaceAuthoritySecret::from_bytes([fixture.authority; 32]);
    let target = SpaceMemberV1::new(
        endpoint(TARGET),
        "target".to_owned(),
        MemberCapabilities::new(true, false),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([fixture.authority; 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(target.clone(), SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let caller = SpaceMemberV1::new(endpoint(CALLER), "caller".to_owned(), fixture.caller)?;
    let mut members = vec![caller, target.clone()];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let mut chain = SpaceChain::from_genesis(genesis)?;
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        2,
        SpaceManifestMembership::new(members, Vec::new()),
    )?
    .sign(&authority)?;
    chain.apply(&first)?;
    if fixture.revoke_caller {
        let revoked = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
            3,
            SpaceManifestMembership::new(
                vec![target],
                vec![SpaceRevocationV1::new(endpoint(CALLER))],
            ),
        )?
        .sign(&authority)?;
        chain.apply(&revoked)?;
    }
    Ok(SpaceAuthorizationView::from_chain(&chain))
}

fn grants(capabilities: &[Capability]) -> MemberCapabilities {
    capabilities.iter().fold(
        MemberCapabilities::new(false, false),
        |grants, capability| grants.with(*capability),
    )
}

fn allowed(
    access: &ServiceAccess,
    spaces: &[SpaceAuthorizationView],
    exposures: &[SpaceExposureView],
) -> bool {
    let request = ServiceAuthorizationRequest::new(
        AuthorizationEndpoints::new(endpoint(CALLER), endpoint(TARGET)),
        access,
        exposures,
    );
    authorize_service(&request, spaces).is_ok()
}

fn docs() -> TestResult<RootName> {
    Ok(RootName::new("docs")?)
}

#[test]
fn one_complete_space_grants_only_the_exposed_root_access() -> TestResult {
    let reader = space(&SpaceFixture {
        authority: 0x41,
        caller: grants(&[Capability::FILES_READ]),
        revoke_caller: false,
    })?;
    let exposure = SpaceExposureView::new(reader.space_id())
        .with_root(ExportedRoot::new(docs()?, RootAccess::ReadWrite));
    let spaces = [reader];
    let exposures = [exposure];
    assert!(allowed(&ServiceAccess::FilesAdmission, &spaces, &exposures));
    assert!(allowed(
        &ServiceAccess::FilesRead(docs()?),
        &spaces,
        &exposures
    ));
    assert!(!allowed(
        &ServiceAccess::FilesWrite(docs()?),
        &spaces,
        &exposures
    ));
    let other = RootName::new("other")?;
    assert!(!allowed(
        &ServiceAccess::FilesRead(other),
        &spaces,
        &exposures
    ));
    assert!(!allowed(&ServiceAccess::Exec, &spaces, &exposures));
    Ok(())
}

#[test]
fn read_only_root_never_permits_mutation() -> TestResult {
    let writer = space(&SpaceFixture {
        authority: 0x42,
        caller: grants(&[Capability::FILES_READ, Capability::FILES_WRITE]),
        revoke_caller: false,
    })?;
    let exposures = [SpaceExposureView::new(writer.space_id())
        .with_root(ExportedRoot::new(docs()?, RootAccess::ReadOnly))];
    let spaces = [writer];
    assert!(allowed(
        &ServiceAccess::FilesRead(docs()?),
        &spaces,
        &exposures
    ));
    assert!(!allowed(
        &ServiceAccess::FilesWrite(docs()?),
        &spaces,
        &exposures
    ));
    Ok(())
}

#[test]
fn a_grant_in_one_space_and_an_exposure_in_another_never_combine() -> TestResult {
    let granted = space(&SpaceFixture {
        authority: 0x43,
        caller: grants(&[Capability::FILES_WRITE, Capability::EXEC]),
        revoke_caller: false,
    })?;
    let exposed = space(&SpaceFixture {
        authority: 0x44,
        caller: grants(&[Capability::FILES_READ]),
        revoke_caller: false,
    })?;
    let exposures = [SpaceExposureView::new(exposed.space_id())
        .with_root(ExportedRoot::new(docs()?, RootAccess::ReadWrite))
        .with_exec()];
    let spaces = [granted, exposed];
    assert!(!allowed(
        &ServiceAccess::FilesWrite(docs()?),
        &spaces,
        &exposures
    ));
    assert!(!allowed(&ServiceAccess::Exec, &spaces, &exposures));
    assert!(allowed(
        &ServiceAccess::FilesRead(docs()?),
        &spaces,
        &exposures
    ));
    Ok(())
}

#[test]
fn revoked_or_unrelated_callers_are_denied_uniformly() -> TestResult {
    let revoked = space(&SpaceFixture {
        authority: 0x45,
        caller: grants(&[Capability::EXEC]),
        revoke_caller: true,
    })?;
    let exposures = [SpaceExposureView::new(revoked.space_id()).with_exec()];
    assert!(!allowed(&ServiceAccess::Exec, &[revoked], &exposures));
    let unrelated = space(&SpaceFixture {
        authority: 0x46,
        caller: grants(&[Capability::EXEC]),
        revoke_caller: false,
    })?;
    let request = ServiceAuthorizationRequest::new(
        AuthorizationEndpoints::new(endpoint(0x77), endpoint(TARGET)),
        &ServiceAccess::Exec,
        &exposures,
    );
    assert!(authorize_service(&request, &[unrelated]).is_err());
    Ok(())
}

#[test]
fn exec_shell_and_proxy_need_both_the_grant_and_the_exposure() -> TestResult {
    let full = space(&SpaceFixture {
        authority: 0x47,
        caller: grants(&[Capability::EXEC, Capability::PROXY]),
        revoke_caller: false,
    })?;
    let rule = ProxyRule::new(
        ProxyHostPattern::Exact(ProxyHost::new("db.internal")?),
        (5432, 5433),
    )?;
    let exposures = [SpaceExposureView::new(full.space_id())
        .with_exec()
        .with_shell()
        .with_proxy_rule(rule)];
    let spaces = [full];
    assert!(allowed(&ServiceAccess::Exec, &spaces, &exposures));
    assert!(!allowed(&ServiceAccess::Shell, &spaces, &exposures));
    assert!(allowed(&ServiceAccess::ProxyAdmission, &spaces, &exposures));
    let db = ProxyHost::new("db.internal")?;
    assert!(allowed(
        &ServiceAccess::ProxyConnect(db.clone(), 5433),
        &spaces,
        &exposures
    ));
    assert!(!allowed(
        &ServiceAccess::ProxyConnect(db, 5434),
        &spaces,
        &exposures
    ));
    let other = ProxyHost::new("10.0.0.1")?;
    assert!(!allowed(
        &ServiceAccess::ProxyConnect(other, 5432),
        &spaces,
        &exposures
    ));
    assert!(!allowed(&ServiceAccess::Exec, &spaces, &[]));
    Ok(())
}

#[test]
fn any_host_rule_is_an_explicit_opt_in_and_port_zero_is_invalid() -> TestResult {
    let any = ProxyRule::new(ProxyHostPattern::Any, (1, 65_535))?;
    assert!(any.allows(&ProxyHost::new("example.org")?, 443));
    assert!(ProxyRule::new(ProxyHostPattern::Any, (0, 10)).is_err());
    assert!(ProxyRule::new(ProxyHostPattern::Any, (20, 10)).is_err());
    Ok(())
}

#[test]
fn remote_service_grants_roundtrip_through_signed_manifests() -> TestResult {
    let all = grants(&[
        Capability::ECHO,
        Capability::PRIVATE_RELAY_PROVIDER,
        Capability::FILES_READ,
        Capability::FILES_WRITE,
        Capability::EXEC,
        Capability::SHELL,
        Capability::PROXY,
        Capability::PROCESS,
        Capability::AGENT,
        Capability::SERVICE,
    ]);
    let view = space(&SpaceFixture {
        authority: 0x48,
        caller: all,
        revoke_caller: false,
    })?;
    for capability in [
        Capability::SHELL,
        Capability::PROXY,
        Capability::FILES_WRITE,
        Capability::PROCESS,
        Capability::AGENT,
        Capability::SERVICE,
    ] {
        assert!(view.allows(endpoint(CALLER), capability));
        assert!(!capability.is_policy_governed());
    }
    assert!(!all.without(Capability::SHELL).allows(Capability::SHELL));
    Ok(())
}

#[test]
fn process_needs_the_grant_and_the_exposure_of_one_single_space() -> TestResult {
    let granted = space(&SpaceFixture {
        authority: 0x49,
        caller: grants(&[Capability::PROCESS]),
        revoke_caller: false,
    })?;
    let exposing = space(&SpaceFixture {
        authority: 0x4a,
        caller: grants(&[Capability::EXEC]),
        revoke_caller: false,
    })?;
    let split = [SpaceExposureView::new(exposing.space_id()).with_process()];
    let spaces = [granted.clone(), exposing];
    assert!(!allowed(&ServiceAccess::Process, &spaces, &split));
    let complete = [SpaceExposureView::new(granted.space_id()).with_process()];
    assert!(allowed(&ServiceAccess::Process, &spaces, &complete));
    let exec_only = [SpaceExposureView::new(granted.space_id()).with_exec()];
    assert!(!allowed(&ServiceAccess::Process, &spaces, &exec_only));
    let revoked = space(&SpaceFixture {
        authority: 0x49,
        caller: grants(&[Capability::PROCESS]),
        revoke_caller: true,
    })?;
    assert!(!allowed(&ServiceAccess::Process, &[revoked], &complete));
    Ok(())
}

#[test]
fn an_agent_needs_the_grant_and_its_own_exposure_in_one_single_space() -> TestResult {
    let helper = AgentId::new("helper")?;
    let other = AgentId::new("other-agent")?;
    let granted = space(&SpaceFixture {
        authority: 0x4b,
        caller: grants(&[Capability::AGENT]),
        revoke_caller: false,
    })?;
    let exposing = space(&SpaceFixture {
        authority: 0x4c,
        caller: grants(&[Capability::PROCESS]),
        revoke_caller: false,
    })?;
    let spaces = [granted.clone(), exposing.clone()];
    let access = ServiceAccess::Agent(helper.clone());
    // The grant in one Space and the exposure in another never combine.
    let split = [SpaceExposureView::new(exposing.space_id()).with_agent(helper.clone())];
    assert!(!allowed(&access, &spaces, &split));
    assert!(!allowed(&ServiceAccess::AgentAdmission, &spaces, &split));
    let complete = [SpaceExposureView::new(granted.space_id()).with_agent(helper.clone())];
    assert!(allowed(&access, &spaces, &complete));
    assert!(allowed(&ServiceAccess::AgentAdmission, &spaces, &complete));
    // Exposing one Agent never exposes another, and a Process exposure is not an Agent one.
    assert!(!allowed(&ServiceAccess::Agent(other), &spaces, &complete));
    let process_only = [SpaceExposureView::new(granted.space_id()).with_process()];
    assert!(!allowed(&access, &spaces, &process_only));
    assert!(!allowed(
        &ServiceAccess::AgentAdmission,
        &spaces,
        &process_only
    ));
    // Without the grant, the exposure alone allows nothing.
    let ungranted = [exposing];
    let exposed_there = [SpaceExposureView::new(ungranted[0].space_id()).with_agent(helper)];
    assert!(!allowed(&access, &ungranted, &exposed_there));
    let revoked = space(&SpaceFixture {
        authority: 0x4b,
        caller: grants(&[Capability::AGENT]),
        revoke_caller: true,
    })?;
    assert!(!allowed(&access, &[revoked], &complete));
    Ok(())
}

#[test]
fn a_hosted_service_needs_the_grant_and_its_own_exposure_in_one_single_space() -> TestResult {
    let example = ServiceName::new("example")?;
    let other = ServiceName::new("other")?;
    let granted = space(&SpaceFixture {
        authority: 0x4d,
        caller: grants(&[Capability::SERVICE]),
        revoke_caller: false,
    })?;
    let exposing = space(&SpaceFixture {
        authority: 0x4e,
        caller: grants(&[Capability::AGENT]),
        revoke_caller: false,
    })?;
    let spaces = [granted.clone(), exposing.clone()];
    let access = ServiceAccess::Hosted(example.clone());
    // The grant in one Space and the exposure in another never combine.
    let split = [SpaceExposureView::new(exposing.space_id()).with_service(example.clone())];
    assert!(!allowed(&access, &spaces, &split));
    let complete = [SpaceExposureView::new(granted.space_id()).with_service(example.clone())];
    assert!(allowed(&access, &spaces, &complete));
    // Exposing one service never exposes another, and an Agent exposure is not a service one.
    assert!(!allowed(&ServiceAccess::Hosted(other), &spaces, &complete));
    let agent_only =
        [SpaceExposureView::new(granted.space_id()).with_agent(AgentId::new("example")?)];
    assert!(!allowed(&access, &spaces, &agent_only));
    // Without the grant, the exposure alone allows nothing.
    let exposed_there = [SpaceExposureView::new(exposing.space_id()).with_service(example)];
    assert!(!allowed(&access, &[exposing], &exposed_there));
    let revoked = space(&SpaceFixture {
        authority: 0x4d,
        caller: grants(&[Capability::SERVICE]),
        revoke_caller: true,
    })?;
    assert!(!allowed(&access, &[revoked], &complete));
    Ok(())
}

#[test]
fn discovery_needs_only_one_shared_space_and_no_exposure() -> TestResult {
    let member = space(&SpaceFixture {
        authority: 0x4f,
        caller: grants(&[]),
        revoke_caller: false,
    })?;
    // No grant and no exposure row at all: membership of both Endpoints is enough.
    assert!(allowed(&ServiceAccess::Discover, &[member], &[]));
    let revoked = space(&SpaceFixture {
        authority: 0x4f,
        caller: grants(&[]),
        revoke_caller: true,
    })?;
    assert!(!allowed(&ServiceAccess::Discover, &[revoked], &[]));
    assert!(!allowed(&ServiceAccess::Discover, &[], &[]));
    Ok(())
}
