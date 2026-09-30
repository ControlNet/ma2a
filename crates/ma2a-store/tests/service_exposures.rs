//! Durable per-Space service exposure and upload staging records.

#[path = "common/support.rs"]
mod support;

use iroh_base::SecretKey;
use ma2a_core::{
    MemberCapabilities, ProtocolError, RequestId, SpaceId, SpaceMemberV1, SpacePolicyV1,
    service::{ExportedRoot, ProxyHost, ProxyHostPattern, ProxyRule, RootAccess, RootName},
};
use ma2a_store::{
    FileRootExposure, LocalExposure, Repository, SpaceCreation, StoreConfig, StoreError,
    TransferStaging,
};
use support::{TempState, TestResult};

use support::TestResultValue as TestValue;

fn create_space(repository: &mut Repository, created_at_ms: u64) -> TestValue<SpaceId> {
    let member = SpaceMemberV1::new(
        SecretKey::from_bytes(&[0x61; 32]).public().into(),
        "owner".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    Ok(repository
        .create_owned_space(&SpaceCreation::new(
            created_at_ms,
            member,
            SpacePolicyV1::phase_one_default(),
        ))?
        .space_id())
}

fn root(name: &str, path: &str, access: RootAccess) -> TestValue<LocalExposure> {
    Ok(LocalExposure::FilesRoot(FileRootExposure::new(
        RootName::new(name)?,
        path.to_owned(),
        access,
    )))
}

#[test]
fn exposures_are_scoped_to_one_space_and_survive_reopen() -> TestResult {
    let state = TempState::new("exposures")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let work = create_space(&mut repository, 1_700_000_000_000)?;
    let family = create_space(&mut repository, 1_700_000_000_001)?;
    let before = repository.revision()?;
    let committed =
        repository.expose_service(work, &root("docs", "/srv/docs", RootAccess::ReadWrite)?)?;
    assert_eq!(committed.revision(), before + 1);
    repository.expose_service(work, &LocalExposure::Exec)?;
    repository.expose_service(family, &root("docs", "/srv/docs", RootAccess::ReadOnly)?)?;
    let rule = ProxyRule::new(
        ProxyHostPattern::Exact(ProxyHost::new("db.internal")?),
        (5432, 5432),
    )?;
    repository.expose_service(family, &LocalExposure::Proxy(rule.clone()))?;
    drop(repository);

    let repository = Repository::open(&config)?;
    let views = repository.service_exposure_views()?;
    let work_view = views
        .iter()
        .find(|view| view.space_id() == Some(work))
        .ok_or("work view")?;
    assert!(work_view.exec());
    assert!(!work_view.shell());
    assert_eq!(work_view.roots().len(), 1);
    let family_view = views
        .iter()
        .find(|view| view.space_id() == Some(family))
        .ok_or("family view")?;
    assert!(!family_view.exec());
    assert_eq!(family_view.proxy_rules(), [rule]);
    assert_eq!(
        family_view.roots().first().map(ExportedRoot::access),
        Some(RootAccess::ReadOnly)
    );
    assert_eq!(
        repository
            .file_root_path(&RootName::new("docs")?)?
            .as_deref(),
        Some("/srv/docs")
    );
    assert_eq!(repository.service_exposures()?.value().len(), 4);
    Ok(())
}

#[test]
fn a_root_name_binds_one_directory_and_is_forgotten_with_its_last_exposure() -> TestResult {
    let state = TempState::new("root-binding")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let first = create_space(&mut repository, 1_700_000_000_000)?;
    let second = create_space(&mut repository, 1_700_000_000_001)?;
    repository.expose_service(first, &root("docs", "/a", RootAccess::ReadOnly)?)?;
    let conflict = repository.expose_service(second, &root("docs", "/b", RootAccess::ReadOnly)?);
    assert!(matches!(
        conflict,
        Err(StoreError::Protocol(error)) if error == ProtocolError::CONFLICT
    ));
    let before = repository.revision()?;
    let absent = repository.unexpose_service(second, &LocalExposure::Shell)?;
    assert!(!absent.value());
    assert_eq!(absent.revision(), before);
    let removed = repository.unexpose_service(first, &root("docs", "/a", RootAccess::ReadOnly)?)?;
    assert!(removed.value());
    assert_eq!(removed.revision(), before + 1);
    assert_eq!(repository.file_root_path(&RootName::new("docs")?)?, None);
    repository.expose_service(second, &root("docs", "/b", RootAccess::ReadOnly)?)?;
    Ok(())
}

#[test]
fn unknown_spaces_cannot_be_exposed_to() -> TestResult {
    let state = TempState::new("unknown-space")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    let unknown = SpaceId::try_from([9_u8; 32].as_slice())?;
    assert!(matches!(
        repository.expose_service(unknown, &LocalExposure::Exec),
        Err(StoreError::SpaceNotFound)
    ));
    Ok(())
}

#[test]
fn staging_records_persist_until_removed() -> TestResult {
    let state = TempState::new("staging")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let request_id = RequestId::try_from([3_u8; 16].as_slice())?;
    repository.record_transfer_staging(TransferStaging::new(
        "/srv/docs/.ma2a-upload-a.part",
        request_id,
        2,
    ))?;
    repository.record_transfer_staging(TransferStaging::new(
        "/srv/docs/.ma2a-upload-b.part",
        request_id,
        1,
    ))?;
    drop(repository);
    let mut repository = Repository::open(&config)?;
    assert_eq!(
        repository.transfer_staging_paths()?,
        [
            "/srv/docs/.ma2a-upload-b.part",
            "/srv/docs/.ma2a-upload-a.part"
        ]
    );
    repository.remove_transfer_staging("/srv/docs/.ma2a-upload-b.part")?;
    assert_eq!(
        repository.transfer_staging_paths()?,
        ["/srv/docs/.ma2a-upload-a.part"]
    );
    Ok(())
}
