//! Private relay advertisements and public fallback separation coverage.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use iroh_base::{RelayUrl, SecretKey};
use ma2a_core::{
    MemberCapabilities, PrivateRelayAdvertisementScope, PrivateRelayAdvertisementV1,
    PrivateRelayAdvertisementValidity, SignedSpaceGenesisV1, SpaceAuthoritySecret,
    SpaceAuthorizationView, SpaceChain, SpaceGenesisIdentity, SpaceGenesisOwner, SpaceGenesisV1,
    SpaceMemberV1, SpacePolicyV1,
};
use ma2a_net::{
    AdvertisementValidationContext, PrivateRelayAdvertisementValidationError,
    PrivateRelayAdvertisementValidator, SpaceAddressLookup,
};
use ma2a_store::{Repository, SpaceRecord, StoreConfig};

const NOW_MS: u64 = 1_700_000_000_000;
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct TempState {
    path: PathBuf,
}

impl TempState {
    fn new(name: &str) -> TestResult<Self> {
        let serial = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("ma2a-relay-{name}-{}-{serial}", std::process::id()));
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

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

fn authorization(
    provider: &SecretKey,
    marker: u8,
    relay_capability: bool,
) -> Result<(SignedSpaceGenesisV1, SpaceAuthorizationView), Box<dyn std::error::Error + Send + Sync>>
{
    let authority = SpaceAuthoritySecret::from_bytes([marker; 32]);
    let member = SpaceMemberV1::new(
        provider.public().into(),
        format!("provider-{marker:02x}"),
        MemberCapabilities::new(true, relay_capability),
    )?;
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([marker.wrapping_add(1); 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(member, SpacePolicyV1::phase_one_default()),
    )
    .sign(&authority)?;
    let chain = SpaceChain::from_genesis(genesis.clone())?;
    Ok((genesis, SpaceAuthorizationView::from_chain(&chain)))
}

struct AdvertisementInput {
    sequence: u64,
    relay_url: RelayUrl,
}

fn advertisement(
    provider: &SecretKey,
    authorization: &SpaceAuthorizationView,
    input: AdvertisementInput,
) -> Result<ma2a_core::SignedPrivateRelayAdvertisementV1, ma2a_core::ProtocolError> {
    PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(authorization.space_id(), provider.public().into()),
        input.relay_url,
        PrivateRelayAdvertisementValidity::new(input.sequence, NOW_MS, NOW_MS + 600_000)?,
    )?
    .sign(provider)
}

#[test]
fn provider_claim_must_be_signed_by_the_claimed_endpoint() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x31; 32]);
    let attacker = SecretKey::from_bytes(&[0x32; 32]);
    let (_genesis, authorization) = authorization(&provider, 0x41, true)?;
    let relay_url = "https://relay.example.invalid".parse()?;
    let unsigned = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(authorization.space_id(), provider.public().into()),
        relay_url,
        PrivateRelayAdvertisementValidity::new(1, NOW_MS, NOW_MS + 600_000)?,
    )?;

    // When
    let result = unsigned.sign(&attacker);

    // Then
    assert_eq!(result, Err(ma2a_core::ProtocolError::INVALID_INPUT));
    Ok(())
}

#[test]
fn private_relay_advertisement_requires_https() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x36; 32]);
    let (_genesis, authorization) = authorization(&provider, 0x46, true)?;

    // When
    let result = PrivateRelayAdvertisementV1::new(
        PrivateRelayAdvertisementScope::new(authorization.space_id(), provider.public().into()),
        "http://relay.example.invalid".parse()?,
        PrivateRelayAdvertisementValidity::new(1, NOW_MS, NOW_MS + 600_000)?,
    );

    // Then
    assert_eq!(result, Err(ma2a_core::ProtocolError::INVALID_INPUT));
    Ok(())
}

#[test]
fn exact_space_capability_and_high_water_rules_are_enforced() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x33; 32]);
    let (genesis, authorization) = authorization(&provider, 0x42, true)?;
    let state = TempState::new("relay-advertisement")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        genesis.space_id(),
        genesis.canonical_bytes().to_vec(),
    ))?;
    let first = advertisement(
        &provider,
        &authorization,
        AdvertisementInput {
            sequence: 4,
            relay_url: "https://relay-a.example.invalid".parse()?,
        },
    )?;
    let fork = advertisement(
        &provider,
        &authorization,
        AdvertisementInput {
            sequence: 4,
            relay_url: "https://relay-b.example.invalid".parse()?,
        },
    )?;
    let rollback = advertisement(
        &provider,
        &authorization,
        AdvertisementInput {
            sequence: 3,
            relay_url: "https://relay-a.example.invalid".parse()?,
        },
    )?;
    let context = AdvertisementValidationContext::new(&authorization, NOW_MS);

    // When
    let accepted = PrivateRelayAdvertisementValidator::validate_and_store(
        &mut repository,
        first.canonical_bytes(),
        context,
    );
    let forked = PrivateRelayAdvertisementValidator::validate_and_store(
        &mut repository,
        fork.canonical_bytes(),
        context,
    );
    let rolled_back = PrivateRelayAdvertisementValidator::validate_and_store(
        &mut repository,
        rollback.canonical_bytes(),
        context,
    );

    // Then
    assert!(accepted.is_ok());
    assert!(matches!(
        forked,
        Err(PrivateRelayAdvertisementValidationError::Fork {
            current_sequence: 4
        })
    ));
    assert!(matches!(
        rolled_back,
        Err(PrivateRelayAdvertisementValidationError::Rollback {
            current_sequence: 4
        })
    ));
    let persisted = repository
        .relay_advertisement(authorization.space_id(), provider.public().into())?
        .ok_or("relay advertisement was not persisted")?;
    assert_eq!(persisted.signed_advertisement(), first.canonical_bytes());
    assert!(
        SpaceAddressLookup::default()
            .resolve_endpoint(provider.public())
            .is_none()
    );
    Ok(())
}

#[test]
fn provider_without_private_relay_capability_is_rejected() -> TestResult {
    // Given
    let provider = SecretKey::from_bytes(&[0x34; 32]);
    let (genesis, authorization) = authorization(&provider, 0x43, false)?;
    let state = TempState::new("relay-capability")?;
    let mut repository = Repository::open(&StoreConfig::new(state.path()))?;
    repository.create_space(&SpaceRecord::new(
        genesis.space_id(),
        genesis.canonical_bytes().to_vec(),
    ))?;
    let signed = advertisement(
        &provider,
        &authorization,
        AdvertisementInput {
            sequence: 1,
            relay_url: "https://relay.example.invalid".parse()?,
        },
    )?;

    // When
    let result = PrivateRelayAdvertisementValidator::validate_and_store(
        &mut repository,
        signed.canonical_bytes(),
        AdvertisementValidationContext::new(&authorization, NOW_MS),
    );

    // Then
    assert!(matches!(
        result,
        Err(PrivateRelayAdvertisementValidationError::UnauthorizedProvider)
    ));
    Ok(())
}
