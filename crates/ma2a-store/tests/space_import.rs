//! Transactional import tests for complete public Space chains.

#[path = "common/support.rs"]
mod support;

use ma2a_core::{
    MemberCapabilities, SpaceAuthoritySecret, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner,
    SpaceGenesisV1, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1,
    SpacePolicyV1,
};
use ma2a_store::{Repository, StoreConfig};
use rusqlite::Connection;
use support::{TempState, TestResult};

#[test]
fn corrupt_later_generation_import_exposes_no_partial_state() -> TestResult {
    let state = TempState::new("atomic-corrupt-space-import")?;
    let config = StoreConfig::new(state.path());
    let chain = chain_through_generation_two()?;
    let mut bytes = chain.export_public()?;
    let final_byte = bytes.last_mut().ok_or("empty export")?;
    *final_byte ^= 1;
    let mut repository = Repository::open(&config)?;

    assert!(repository.import_space_chain(&bytes).is_err());
    assert_eq!(repository.revision()?, 0);
    assert_eq!(repository.load_space_chain(chain.space_id())?, None);
    drop(repository);

    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM spaces", [], |row| row
            .get::<_, u32>(0))?,
        0
    );
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM manifests", [], |row| row
            .get::<_, u32>(0))?,
        0
    );
    Ok(())
}

fn chain_through_generation_two() -> Result<SpaceChain, Box<dyn std::error::Error + Send + Sync>> {
    let secret = SpaceAuthoritySecret::from_bytes([0x71; 32]);
    let owner = member(0x66)?;
    let peer = member(0x77)?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x72; 32], 10, secret.public_key())?,
        SpaceGenesisOwner::new(owner.clone(), SpacePolicyV1::phase_one_default()),
    )
    .sign(&secret)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        11,
        SpaceManifestMembership::new(sorted_members(peer, owner), vec![]),
    )?
    .sign(&secret)?;
    chain.apply(&first)?;
    let second = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 2, chain.latest_hash()),
        12,
        SpaceManifestMembership::new(first.members().to_vec(), vec![]),
    )?
    .sign(&secret)?;
    chain.apply(&second)?;
    Ok(chain)
}

fn sorted_members(left: SpaceMemberV1, right: SpaceMemberV1) -> Vec<SpaceMemberV1> {
    if left.endpoint_id() < right.endpoint_id() {
        vec![left, right]
    } else {
        vec![right, left]
    }
}

fn member(marker: u8) -> Result<SpaceMemberV1, ma2a_core::ProtocolError> {
    let bytes = match marker {
        0x66 => [
            0xdb, 0x99, 0x5f, 0xe2, 0x51, 0x69, 0xd1, 0x41, 0xca, 0xb9, 0xbb, 0xba, 0x92, 0xba,
            0xa0, 0x1f, 0x9f, 0x2e, 0x1e, 0xce, 0x7d, 0xf4, 0xcb, 0x2a, 0xc0, 0x51, 0x90, 0xf3,
            0x7f, 0xcc, 0x1f, 0x9d,
        ],
        0x77 => [
            0x21, 0x52, 0xf8, 0xd1, 0x9b, 0x79, 0x1d, 0x24, 0x45, 0x32, 0x42, 0xe1, 0x5f, 0x2e,
            0xab, 0x6c, 0xb7, 0xcf, 0xfa, 0x7b, 0x6a, 0x5e, 0xd3, 0x00, 0x97, 0x96, 0x0e, 0x06,
            0x98, 0x81, 0xdb, 0x12,
        ],
        _ => return Err(ma2a_core::ProtocolError::INVALID_INPUT),
    };
    SpaceMemberV1::new(
        ma2a_core::EndpointId::try_from(bytes.as_slice())?,
        format!("member-{marker:02x}"),
        MemberCapabilities::new(true, marker == 0x66),
    )
}
