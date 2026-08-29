//! Independent golden-vector tests for signed Space objects.

#[path = "support/space_vectors.rs"]
mod space_vectors;

use std::fmt::Write as _;

use ma2a_core::{
    EnrollmentPage, MAX_ENROLLMENT_ARTIFACTS_PER_PAGE, MAX_ENROLLMENT_PAGE_BYTES,
    ManifestApplyOutcome, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceAuthoritySecret,
    SpaceChain, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1,
    validate_enrollment_pages,
};

use space_vectors::{signed_genesis, signed_manifest};

#[test]
fn genesis_matches_independent_golden_vector() -> Result<(), Box<dyn std::error::Error>> {
    // Given
    let golden_envelope = golden("genesis.cbor")?;
    let golden_body = golden("genesis-body.cbor")?;

    // When
    let signed = signed_genesis()?;
    let decoded = SignedSpaceGenesisV1::from_canonical_bytes(&golden_envelope)?;

    // Then
    assert_eq!(signed.canonical_bytes(), golden_envelope);
    assert_eq!(signed.canonical_body_bytes(), golden_body);
    assert_eq!(decoded, signed);
    assert_eq!(
        hex(decoded.space_id().as_bytes())?,
        golden_text("space-id.hex")?
    );
    assert_eq!(
        hex(&decoded.signature_bytes())?,
        golden_text("genesis-signature.hex")?
    );
    Ok(())
}

#[test]
fn manifest_matches_independent_golden_vector() -> Result<(), Box<dyn std::error::Error>> {
    // Given
    let genesis = signed_genesis()?;
    let golden = golden("manifest-1.cbor")?;

    // When
    let signed = signed_manifest(&genesis)?;
    let decoded = SignedSpaceManifestV1::from_canonical_bytes(&golden, genesis.authority())?;

    // Then
    assert_eq!(signed.canonical_bytes(), golden);
    assert_eq!(decoded, signed);
    assert_eq!(
        hex(&decoded.manifest_hash())?,
        golden_text("manifest-1-hash.hex")?
    );
    assert_eq!(
        hex(&decoded.signature_bytes())?,
        golden_text("manifest-1-signature.hex")?
    );
    Ok(())
}

#[test]
fn public_chain_round_trips_generation_zero_through_one() -> Result<(), Box<dyn std::error::Error>>
{
    // Given
    let genesis = signed_genesis()?;
    let manifest = signed_manifest(&genesis)?;
    let mut chain = SpaceChain::from_genesis(genesis)?;
    assert_eq!(chain.apply(&manifest)?, ManifestApplyOutcome::ADVANCED);

    // When
    let exported = chain.export_public()?;
    let imported = SpaceChain::import_public(&exported)?;

    // Then
    assert_eq!(imported, chain);
    assert_eq!(imported.latest_generation(), 1);
    assert_eq!(imported.manifests().len(), 1);
    assert!(
        !exported
            .windows(32)
            .any(|window| window == space_vectors::AUTHORITY_SECRET)
    );
    Ok(())
}

#[test]
fn public_chain_round_trips_beyond_single_object_bound() -> Result<(), Box<dyn std::error::Error>> {
    let genesis = signed_genesis()?;
    let secret = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let mut chain = SpaceChain::from_genesis(genesis)?;
    for generation in 1..=180 {
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), generation, chain.latest_hash()),
            generation,
            SpaceManifestMembership::new(vec![space_vectors::member(0x66, true)?], vec![]),
        )?
        .sign(&secret)?;
        chain.apply(&manifest)?;
    }

    let exported = chain.export_public()?;
    assert!(exported.len() > 32_768);
    assert_eq!(SpaceChain::import_public(&exported)?, chain);
    for generation in 181..=256 {
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), generation, chain.latest_hash()),
            generation,
            SpaceManifestMembership::new(vec![space_vectors::member(0x66, true)?], vec![]),
        )?
        .sign(&secret)?;
        chain.apply(&manifest)?;
    }
    assert_eq!(
        chain.export_public(),
        Err(ma2a_core::ProtocolError::INVALID_INPUT)
    );
    Ok(())
}

#[test]
fn maximum_generation_chain_paginates_without_an_aggregate_frame()
-> Result<(), Box<dyn std::error::Error>> {
    let secret = SpaceAuthoritySecret::from_bytes(space_vectors::AUTHORITY_SECRET);
    let mut chain = SpaceChain::from_genesis(signed_genesis()?)?;
    for generation in 1..=255 {
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), generation, chain.latest_hash()),
            generation,
            SpaceManifestMembership::new(vec![space_vectors::member(0x66, true)?], vec![]),
        )?
        .sign(&secret)?;
        chain.apply(&manifest)?;
    }

    let pages = EnrollmentPage::paginate(&chain)?;

    assert_eq!(
        pages.len(),
        255_usize.div_ceil(MAX_ENROLLMENT_ARTIFACTS_PER_PAGE)
    );
    assert!(pages.iter().all(|page| {
        page.encode()
            .is_ok_and(|bytes| bytes.len() <= MAX_ENROLLMENT_PAGE_BYTES)
    }));
    assert_eq!(validate_enrollment_pages(&pages)?, chain);
    Ok(())
}

fn hex(bytes: &[u8]) -> Result<String, std::fmt::Error> {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}")?;
    }
    Ok(output)
}

fn golden(name: &str) -> Result<Vec<u8>, std::io::Error> {
    std::fs::read(format!(
        "{}/testdata/space-v1/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn golden_text(name: &str) -> Result<String, std::io::Error> {
    Ok(std::fs::read_to_string(format!(
        "{}/testdata/space-v1/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))?
    .trim()
    .to_owned())
}
