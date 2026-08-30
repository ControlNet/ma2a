use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use ma2a_core::{
    ControlArtifactKind, ControlArtifactV1, ControlPageV1, MemberCapabilities,
    SpaceAuthoritySecret, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{Repository, StoreConfig};

use super::{PageApplication, apply_pages};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
static NEXT_STATE: AtomicU64 = AtomicU64::new(0);

struct TempState(PathBuf);

impl TempState {
    fn new() -> TestResultValue<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-control-staging-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

type TestResultValue<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn invalid_later_artifact_leaves_manifest_and_revision_unchanged() -> TestResult {
    // Given
    let state = TempState::new()?;
    let mut repository = Repository::open(&StoreConfig::new(&state.0))?;
    let authority = SpaceAuthoritySecret::from_bytes([0x31; 32]);
    let local_secret = SpaceAuthoritySecret::from_bytes([0x32; 32]);
    let remote_secret = SpaceAuthoritySecret::from_bytes([0x33; 32]);
    let local = ma2a_core::EndpointId::try_from(local_secret.public_key().as_bytes().as_slice())?;
    let remote = ma2a_core::EndpointId::try_from(remote_secret.public_key().as_bytes().as_slice())?;
    let local_member = SpaceMemberV1::new(
        local,
        "local".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let remote_member = SpaceMemberV1::new(
        remote,
        "remote".to_owned(),
        MemberCapabilities::new(true, true),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x34; 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(local_member.clone(), SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let mut members = vec![local_member, remote_member];
    members.sort_by_key(SpaceMemberV1::endpoint_id);
    let mut chain = SpaceChain::from_genesis(genesis)?;
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        100,
        SpaceManifestMembership::new(members.clone(), vec![]),
    )?
    .sign(&authority)?;
    chain.apply(&first)?;
    repository.persist_space_chain(&chain)?;
    let next = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        101,
        SpaceManifestMembership::new(members, vec![]),
    )?
    .sign(&authority)?;
    let page = ControlPageV1::new(
        chain.space_id(),
        false,
        vec![
            ControlArtifactV1::new(
                ControlArtifactKind::MANIFEST,
                next.canonical_bytes().to_vec(),
            )?,
            ControlArtifactV1::new(ControlArtifactKind::ADDRESS_RECORD, vec![1])?,
        ],
    )?;
    let shared = repository.control_spaces_between(local, remote)?;
    let revision = repository.revision()?;

    // When
    let result = apply_pages(&mut repository, PageApplication::new(&shared, &[page], 50));

    // Then
    assert_eq!(result, Err(ma2a_net::ControlRejection::Invalid));
    assert_eq!(repository.revision()?, revision);
    assert_eq!(
        repository
            .load_space_chain(chain.space_id())?
            .ok_or("chain missing")?
            .latest_generation(),
        1
    );
    Ok(())
}
