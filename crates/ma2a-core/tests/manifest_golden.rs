//! Independent golden-vector tests for signed Space objects.

#[path = "support/space_vectors.rs"]
mod space_vectors;

use std::fmt::Write as _;

use ma2a_core::{
    EnrollmentPage, MAX_ENROLLMENT_ARTIFACTS_PER_PAGE, MAX_ENROLLMENT_PAGE_BYTES,
    MAX_ENROLLMENT_PAGES, MAX_MEMBER_LABEL_LEN, MAX_SPACE_MEMBERS, ManifestApplyOutcome,
    MemberCapabilities, SignedSpaceGenesisV1, SignedSpaceManifestV1, SpaceAuthoritySecret,
    SpaceChain, SpaceManifestLink, SpaceManifestMembership, SpaceManifestV1, SpaceMemberV1,
    SpaceRevocationV1, validate_enrollment_pages,
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
    let initial = chain.genesis().genesis().initial_member().clone();
    let mut first_members = vec![initial];
    for seed in 1..MAX_SPACE_MEMBERS {
        first_members.push(maximum_label_member(u8::try_from(seed)?)?);
    }
    first_members.sort_by_key(SpaceMemberV1::endpoint_id);
    first_members.dedup_by_key(|member| member.endpoint_id());
    assert_eq!(first_members.len(), MAX_SPACE_MEMBERS);
    let mut second_members = Vec::with_capacity(MAX_SPACE_MEMBERS);
    for seed in (MAX_SPACE_MEMBERS * 2)..(MAX_SPACE_MEMBERS * 3) {
        second_members.push(maximum_label_member(u8::try_from(seed)?)?);
    }
    second_members.sort_by_key(SpaceMemberV1::endpoint_id);
    second_members.dedup_by_key(|member| member.endpoint_id());
    assert_eq!(second_members.len(), MAX_SPACE_MEMBERS);
    let revocations = first_members
        .iter()
        .map(|member| SpaceRevocationV1::new(member.endpoint_id()))
        .collect::<Vec<_>>();
    let first = SpaceManifestV1::new(
        SpaceManifestLink::new(chain.space_id(), 1, chain.latest_hash()),
        1,
        SpaceManifestMembership::new(first_members, vec![]),
    )?
    .sign(&secret)?;
    chain.apply(&first)?;
    for generation in 2..=255 {
        let manifest = SpaceManifestV1::new(
            SpaceManifestLink::new(chain.space_id(), generation, chain.latest_hash()),
            generation,
            SpaceManifestMembership::new(second_members.clone(), revocations.clone()),
        )?
        .sign(&secret)?;
        chain.apply(&manifest)?;
    }

    let pages = EnrollmentPage::paginate(&chain)?;

    assert_eq!(
        pages.len(),
        255_usize.div_ceil(MAX_ENROLLMENT_ARTIFACTS_PER_PAGE)
    );
    assert_eq!(pages.len(), MAX_ENROLLMENT_PAGES);
    assert!(pages.iter().all(|page| {
        page.encode()
            .is_ok_and(|bytes| bytes.len() <= MAX_ENROLLMENT_PAGE_BYTES)
    }));
    let aggregate_bytes = pages.iter().try_fold(0_usize, |total, page| {
        page.encode().map(|bytes| total + bytes.len())
    })?;
    assert!(aggregate_bytes < 4 * 1024 * 1024);
    assert_eq!(validate_enrollment_pages(&pages)?, chain);
    Ok(())
}

fn maximum_label_member(seed: u8) -> Result<SpaceMemberV1, Box<dyn std::error::Error>> {
    let verifying_key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]).verifying_key();
    Ok(SpaceMemberV1::new(
        ma2a_core::EndpointId::try_from(verifying_key.to_bytes().as_slice())?,
        "m".repeat(MAX_MEMBER_LABEL_LEN),
        MemberCapabilities::new(true, true),
    )?)
}

#[test]
fn enrollment_page_count_above_protocol_maximum_is_rejected()
-> Result<(), Box<dyn std::error::Error>> {
    // Given
    let chain = SpaceChain::from_genesis(signed_genesis()?)?;
    let mut encoded = EnrollmentPage::paginate(&chain)?
        .into_iter()
        .next()
        .ok_or("page missing")?
        .encode()?;
    encoded
        .get_mut(3..5)
        .ok_or("page count field missing")?
        .copy_from_slice(&33_u16.to_be_bytes());

    // When
    let decoded = EnrollmentPage::decode(&encoded);

    // Then
    assert!(decoded.is_err());
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
