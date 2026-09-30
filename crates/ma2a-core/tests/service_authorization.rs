//! Single-complete-Space authorization for built-in remote services.

use iroh_base::SecretKey;
use ma2a_core::{
    AuthorizationEndpoints, Capability, EndpointId, MemberCapabilities, SpaceAuthoritySecret,
    SpaceAuthorizationView, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1,
    SpaceRevocationV1,
    service::{
        ExportedRoot, ProxyHost, ProxyHostPattern, ProxyRule, RootAccess, RootName, ServiceAccess,
        ServiceAuthorizationRequest, SpaceExposureView, authorize_service,
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
    ] {
        assert!(view.allows(endpoint(CALLER), capability));
        assert!(!capability.is_policy_governed());
    }
    assert!(!all.without(Capability::SHELL).allows(Capability::SHELL));
    Ok(())
}
