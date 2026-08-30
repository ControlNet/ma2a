use ma2a_store::{
    AddressRecordOutcome, AddressRecordTarget, AddressRecordValidation, PasswordReset,
    RelayConfiguration, RelayObservation, RelayTransportConfiguration, Repository,
    RuntimeMetadataUpdate, SessionDigests, SessionRecord, SessionTimestamps, StoreConfig,
    ValidatedAddressRecord, derive_password_verifier,
};
use rusqlite::Connection;

use super::{space_id, support::TempState};
use crate::support::TestResult;

#[test]
fn address_rejects_stale_sequences() -> TestResult {
    // Given
    let state = TempState::new("highest-sequences")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&super::space_fixture::space_record()?)?;
    let first = validated_address(&repository, 4, 0x71)?;
    let stale_record = validated_address(&repository, 3, 0x72)?;
    // When
    let address = repository.advance_validated_address(&first)?;
    let stale_address = repository.advance_validated_address(&stale_record)?;
    // Then
    assert!(matches!(address, AddressRecordOutcome::Advanced { .. }));
    assert_eq!(
        stale_address,
        AddressRecordOutcome::Rollback {
            current_sequence: 4
        }
    );
    Ok(())
}

#[test]
fn address_high_water_rejects_forks_and_rollbacks_after_reopen() -> TestResult {
    // Given
    let state = TempState::new("address-high-water-reopen")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&super::space_fixture::space_record()?)?;
    let first = validated_address(&repository, 4, 0x71)?;
    assert!(matches!(
        repository.advance_validated_address(&first)?,
        AddressRecordOutcome::Advanced { .. }
    ));
    drop(repository);
    let mut repository = Repository::open(&config)?;

    // When
    let replay = repository.advance_validated_address(&first)?;
    let fork = repository.advance_validated_address(&validated_address(&repository, 4, 0x72)?)?;
    let rollback =
        repository.advance_validated_address(&validated_address(&repository, 3, 0x73)?)?;

    // Then
    assert_eq!(
        replay,
        AddressRecordOutcome::Idempotent {
            current_sequence: 4
        }
    );
    assert_eq!(
        fork,
        AddressRecordOutcome::Fork {
            current_sequence: 4
        }
    );
    assert_eq!(
        rollback,
        AddressRecordOutcome::Rollback {
            current_sequence: 4
        }
    );
    assert_eq!(
        repository
            .address_record(space_id()?, first.record().record().endpoint_id())?
            .ok_or("persisted address record was missing")?
            .signed_record(),
        first.record().canonical_bytes()
    );
    Ok(())
}

fn validated_address(
    repository: &Repository,
    sequence: u64,
    payload: u8,
) -> Result<ValidatedAddressRecord, Box<dyn std::error::Error + Send + Sync>> {
    let secret = iroh_base::SecretKey::from_bytes(&[0x43; 32]);
    let space_id = space_id()?;
    let chain = repository
        .load_space_chain(space_id)?
        .ok_or("address Space chain missing")?;
    let authorization = ma2a_core::SpaceAuthorizationView::from_chain(&chain);
    let signed = ma2a_core::SpaceAddressRecordV1::new(
        ma2a_core::AddressRecordScope::new(space_id, secret.public().into()),
        ma2a_core::AddressRecordValidity::new(sequence, 10, 20)?,
        ma2a_core::AddressEndpointDataV1::new(vec![iroh_base::TransportAddr::Ip(
            std::net::SocketAddr::from(([127, 0, 0, 1], u16::from(payload))),
        )])?,
    )
    .sign(&secret)?;
    Ok(ValidatedAddressRecord::parse(
        signed.canonical_bytes(),
        AddressRecordValidation::new(
            AddressRecordTarget::new(space_id, secret.public().into()),
            &authorization,
            15,
        ),
    )?)
}

#[test]
fn password_reset_revokes_existing_sessions_in_one_revision() -> TestResult {
    // Given
    let state = TempState::new("password-reset")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_session(&SessionRecord::new(
        SessionDigests::new([10; 32], [11; 32]),
        1,
        SessionTimestamps::new([1, 1], [100, 100]),
    ))?;

    // When
    repository.change_password(
        ma2a_store::PasswordTransition::Set,
        &PasswordReset {
            verifier: derive_password_verifier(b"atomic-reset-passphrase-9!")?,
            verifier_version: 1,
            now_ms: 50,
        },
    )?;
    drop(repository);

    // Then
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM sessions WHERE revoked_at_ms = 50",
            [],
            |row| row.get::<_, u32>(0)
        )?,
        1
    );
    assert!(
        connection
            .query_row("SELECT password_verifier FROM ui_credentials", [], |row| {
                row.get::<_, Vec<u8>>(0)
            })?
            .starts_with(b"$argon2id$v=19$m=19456,t=2,p=1$")
    );
    Ok(())
}

#[test]
fn signed_membership_and_relay_metadata_are_persisted() -> TestResult {
    // Given
    let state = TempState::new("public-state")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&super::space_fixture::space_record()?)?;
    let genesis = super::space_fixture::signed_space_genesis()?;
    repository.advance_manifest(&super::manifest_advance(1, genesis.chain_hash())?)?;

    // When
    repository.record_runtime_metadata(&RuntimeMetadataUpdate {
        boot_id: [12; 16],
        last_shutdown_clean: false,
        observed_at_ms: 70,
    })?;
    let relay_configuration = RelayConfiguration {
        public_fallback_enabled: true,
        public_relay_urls: vec![
            "https://relay-a.invalid".to_owned(),
            "https://relay-b.invalid".to_owned(),
        ],
        private_provider_enabled: true,
        listener_address: Some("127.0.0.1:443".to_owned()),
        private_relay_url: Some("https://private-relay.invalid".to_owned()),
        served_spaces: vec![space_id()?],
        transport: Some(RelayTransportConfiguration::NativeTls {
            certificate_path: "/run/ma2a/relay.cert.pem".to_owned(),
            private_key_path: "/run/ma2a/relay.key.pem".to_owned(),
        }),
    };
    repository.set_relay_configuration(&relay_configuration)?;
    assert_eq!(repository.reserve_private_relay_sequence()?, 1);
    repository.record_relay_observation(&RelayObservation {
        relay_url: "https://relay.invalid".to_owned(),
        observed_at_ms: 70,
        expires_at_ms: 90,
        reachable: true,
        latency_ms: Some(4),
        observed_state: b"observed".to_vec(),
    })?;
    drop(repository);

    // Then
    let mut repository = Repository::open(&config)?;
    assert_eq!(repository.relay_configuration()?, relay_configuration);
    assert_eq!(repository.reserve_private_relay_sequence()?, 2);
    drop(repository);
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM members", [], |row| row
            .get::<_, u32>(0))?,
        1
    );
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM member_revocations", [], |row| row
            .get::<_, u32>(0))?,
        0
    );
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM relay_observations", [], |row| row
            .get::<_, u32>(0))?,
        1
    );
    assert!(connection.query_row(
        "SELECT public_fallback_enabled FROM relay_configuration",
        [],
        |row| row.get::<_, bool>(0)
    )?);
    assert_eq!(
        connection.query_row("SELECT boot_id FROM runtime_metadata", [], |row| row
            .get::<_, Vec<u8>>(0))?,
        [12; 16]
    );
    Ok(())
}
