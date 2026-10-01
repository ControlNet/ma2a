//! Durable hosted service registrations and their per-Space exposure.

#[path = "common/support.rs"]
mod support;

use iroh_base::SecretKey;
use ma2a_core::{
    MemberCapabilities, SpaceId, SpaceMemberV1, SpacePolicyV1,
    service::{Interaction, OperationName, ServiceManifest, ServiceName, ServiceVersion},
};
use ma2a_store::{LocalExposure, Repository, ServiceRegistration, SpaceCreation, StoreConfig};
use support::{TempState, TestResult, TestResultValue};

fn open(name: &str) -> TestResultValue<(TempState, Repository)> {
    let state = TempState::new(name)?;
    let repository = Repository::open(&StoreConfig::new(state.path()))?;
    Ok((state, repository))
}

fn manifest(description: &str, max_calls: u8) -> TestResultValue<ServiceManifest> {
    let operations = vec![(OperationName::new("echo")?, Interaction::Unary)];
    let versions = vec![
        ServiceVersion::new(1, operations.clone())?,
        ServiceVersion::new(2, operations)?,
    ];
    Ok(ServiceManifest::new(description, versions, max_calls)?)
}

fn example() -> TestResultValue<ServiceName> {
    Ok(ServiceName::new("example")?)
}

fn create_space(repository: &mut Repository) -> TestResultValue<SpaceId> {
    let member = SpaceMemberV1::new(
        SecretKey::from_bytes(&[0x71; 32]).public().into(),
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    Ok(repository
        .create_owned_space(&SpaceCreation::new(
            1,
            member,
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id())
}

#[test]
fn a_registration_changes_only_while_its_service_is_stopped() -> TestResult {
    let (_state, mut repository) = open("service-register")?;
    let name = example()?;
    let first = repository
        .register_service((&name, &manifest("First.", 4)?), 10)?
        .into_value();
    let ServiceRegistration::Registered(record, true) = first else {
        return Err(format!("expected a new registration, got {first:?}").into());
    };
    assert!(!record.enabled, "a new service starts stopped");
    assert_eq!(record.manifest.max_calls(), 4);

    let replaced = repository
        .register_service((&name, &manifest("Second.", 2)?), 20)?
        .into_value();
    let ServiceRegistration::Registered(record, false) = replaced else {
        return Err(format!("expected a replacement, got {replaced:?}").into());
    };
    assert_eq!(record.manifest.description(), "Second.");
    assert_eq!((record.created_at_ms, record.updated_at_ms), (10, 20));

    let started = repository
        .set_service_enabled(&name, (true, 30))?
        .into_value();
    assert!(started.is_some_and(|record| record.enabled));
    assert_eq!(
        repository
            .register_service((&name, &manifest("Third.", 1)?), 40)?
            .into_value(),
        ServiceRegistration::Active
    );
    assert_eq!(
        repository.unregister_service(&name)?.into_value(),
        ServiceRegistration::Active
    );
    repository.set_service_enabled(&name, (false, 50))?;
    assert!(matches!(
        repository.unregister_service(&name)?.into_value(),
        ServiceRegistration::Registered(_, false)
    ));
    assert_eq!(
        repository.unregister_service(&name)?.into_value(),
        ServiceRegistration::Missing
    );
    assert!(repository.hosted_service(&name)?.is_none());
    assert!(
        repository
            .set_service_enabled(&name, (true, 60))?
            .into_value()
            .is_none()
    );
    Ok(())
}

#[test]
fn registrations_survive_reopen_and_page_by_name() -> TestResult {
    let state = TempState::new("service-pages")?;
    let config = StoreConfig::new(state.path());
    {
        let mut repository = Repository::open(&config)?;
        for name in ["delta", "alpha", "charlie", "bravo"] {
            repository.register_service((&ServiceName::new(name)?, &manifest(name, 1)?), 1)?;
        }
        repository.set_service_enabled(&ServiceName::new("charlie")?, (true, 2))?;
    }
    let repository = Repository::open(&config)?;
    let first = repository.hosted_services((None, 2))?.into_value();
    let names: Vec<&str> = first.iter().map(|record| record.name.as_str()).collect();
    assert_eq!(names, ["alpha", "bravo"]);
    let after = ServiceName::new("bravo")?;
    let rest = repository.hosted_services((Some(&after), 10))?.into_value();
    let names: Vec<&str> = rest.iter().map(|record| record.name.as_str()).collect();
    assert_eq!(names, ["charlie", "delta"]);
    assert!(rest.first().is_some_and(|record| record.enabled));
    Ok(())
}

#[test]
fn a_service_exposure_is_its_own_kind_and_reaches_the_authorization_view() -> TestResult {
    let (_state, mut repository) = open("service-exposure")?;
    let space = create_space(&mut repository)?;
    let exposure = LocalExposure::Service(example()?);
    repository.expose_service(space, &exposure)?;
    // An Agent of the same name is a different exposure.
    repository.expose_service(
        space,
        &LocalExposure::Agent(ma2a_core::service::AgentId::new("example")?),
    )?;
    let records = repository.service_exposures()?.into_value();
    assert_eq!(records.len(), 2);
    let views = repository.service_exposure_views()?;
    let view = views.first().ok_or("missing view")?;
    assert_eq!(view.services(), [example()?]);
    assert!(repository.unexpose_service(space, &exposure)?.into_value());
    assert!(
        repository
            .service_exposure_views()?
            .iter()
            .all(|view| view.services().is_empty())
    );
    Ok(())
}
