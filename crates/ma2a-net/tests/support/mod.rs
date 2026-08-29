use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use iroh_base::SecretKey;
use ma2a_core::{
    MemberCapabilities, SignedSpaceGenesisV1, SpaceAuthoritySecret, SpaceAuthorizationView,
    SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1, SpaceMemberV1,
    SpacePolicyV1,
};

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempState {
    path: PathBuf,
}

impl TempState {
    pub(crate) fn new(name: &str) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-net-{name}-{}-{serial}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct SpaceFixture {
    pub(crate) genesis: SignedSpaceGenesisV1,
    pub(crate) authorization: SpaceAuthorizationView,
}

pub(crate) fn space_fixture(
    endpoint_secret: &SecretKey,
    marker: u8,
) -> Result<SpaceFixture, Box<dyn std::error::Error + Send + Sync>> {
    let authority = SpaceAuthoritySecret::from_bytes([marker; 32]);
    let member = SpaceMemberV1::new(
        endpoint_secret.public().into(),
        format!("endpoint-{marker:02x}"),
        MemberCapabilities::new(true, false),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([marker.wrapping_add(1); 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let chain = SpaceChain::from_genesis(genesis.clone())?;
    Ok(SpaceFixture {
        genesis,
        authorization: SpaceAuthorizationView::from_chain(&chain),
    })
}
