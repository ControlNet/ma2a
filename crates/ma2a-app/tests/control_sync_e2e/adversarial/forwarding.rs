use ma2a_core::{
    ControlArtifactKind, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SpaceChain, SpaceManifestLink, SpaceManifestMembership,
    SpaceManifestV1,
};
use ma2a_store::Repository;

use super::super::control_sync_e2e_support::{TestResult, control_fixture};
use support::{AddressArtifact, address_record, artifact_page, authority_secret, reject_page};

#[path = "forwarding/support.rs"]
mod support;

const NOW_MS: u64 = 1_700_000_000_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_peer_manifest_fork_preserves_all_control_state() -> TestResult {
    let fixture = control_fixture().await?;
    let chain = Repository::open(&fixture.candidate_config())?
        .load_space_chain(fixture.shared_spaces[0])?
        .ok_or("candidate chain missing")?;
    let genesis = SpaceChain::from_genesis(chain.genesis().clone())?;
    let authority = authority_secret(&fixture.owner_config(), chain.space_id())?;
    let fork = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, genesis.latest_hash()),
        NOW_MS + 99,
        SpaceManifestMembership::new(chain.members().to_vec(), chain.revocations().to_vec()),
    )?
    .sign(&authority)?;
    reject_page(
        fixture,
        artifact_page(
            chain.space_id(),
            ControlArtifactKind::MANIFEST,
            fork.canonical_bytes(),
        )?,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_peer_lower_address_sequence_preserves_lookup_candidate() -> TestResult {
    let fixture = control_fixture().await?;
    let port = Repository::open(&fixture.owner_config())?
        .endpoint_bind_port()?
        .ok_or("owner port missing")?;
    let record = address_record(
        &fixture,
        &AddressArtifact {
            space_id: fixture.shared_spaces[0],
            sequence: 0,
            port,
            issued_at_ms: NOW_MS,
            expires_at_ms: NOW_MS + 1,
        },
    )?;
    reject_page(
        fixture,
        artifact_page(
            record.record().space_id(),
            ControlArtifactKind::ADDRESS_RECORD,
            record.canonical_bytes(),
        )?,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_peer_expired_relay_preserves_relay_high_water() -> TestResult {
    let fixture = control_fixture().await?;
    let advertisement = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(
            fixture.shared_spaces[0],
            fixture.owner_secret.public().into(),
        ),
        "https://expired.example.invalid".parse()?,
        PrivateRelayAdvertisementValidity::new(2, NOW_MS - 600_000, NOW_MS)?,
    )?
    .sign(&fixture.owner_secret)?;
    reject_page(
        fixture,
        artifact_page(
            advertisement.advertisement().space_id(),
            ControlArtifactKind::RELAY_ADVERTISEMENT,
            advertisement.canonical_bytes(),
        )?,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_peer_relay_signer_provider_mismatch_preserves_authorization() -> TestResult {
    let fixture = control_fixture().await?;
    let space_id = fixture.shared_spaces[0];
    let mut bytes = fixture
        .owner_advertisements
        .first()
        .ok_or("owner advertisement missing")?
        .canonical_bytes()
        .to_vec();
    let signature = fixture
        .candidate_secret
        .sign(b"provider-mismatch")
        .to_bytes();
    let start = bytes
        .len()
        .checked_sub(signature.len())
        .ok_or("signature missing")?;
    bytes
        .get_mut(start..)
        .ok_or("signature range missing")?
        .copy_from_slice(&signature);
    reject_page(
        fixture,
        artifact_page(space_id, ControlArtifactKind::RELAY_ADVERTISEMENT, &bytes)?,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn valid_peer_private_space_injection_preserves_shared_authorization() -> TestResult {
    let fixture = control_fixture().await?;
    let port = Repository::open(&fixture.owner_config())?
        .endpoint_bind_port()?
        .ok_or("owner port missing")?;
    let record = address_record(
        &fixture,
        &AddressArtifact {
            space_id: fixture.private_space,
            sequence: 1,
            port,
            issued_at_ms: NOW_MS,
            expires_at_ms: NOW_MS + 1,
        },
    )?;
    reject_page(
        fixture,
        artifact_page(
            record.record().space_id(),
            ControlArtifactKind::ADDRESS_RECORD,
            record.canonical_bytes(),
        )?,
    )
    .await
}
