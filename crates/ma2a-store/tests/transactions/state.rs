use ma2a_store::{
    AddressAdvance, AddressRecordOutcome, PasswordReset, RelayConfiguration, RelayObservation,
    RelayTransportConfiguration, Repository, RuntimeMetadataUpdate, SessionDigests, SessionRecord,
    SessionTimestamps, StoreConfig, derive_password_verifier,
};
use rusqlite::Connection;

use super::{endpoint_id, space_id, support::TempState};
use crate::support::TestResult;

#[test]
fn address_rejects_stale_sequences() -> TestResult {
    // Given
    let state = TempState::new("highest-sequences")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_space(&super::space_fixture::space_record()?)?;
    let endpoint = endpoint_id()?;
    // When
    let address = repository.advance_address(&AddressAdvance {
        space_id: space_id()?,
        endpoint_id: endpoint,
        sequence: 4,
        issued_at_ms: 1,
        expires_at_ms: 20,
        record_hash: [7; 32],
        signed_record: b"address".to_vec(),
    })?;
    let stale_address = repository.advance_address(&AddressAdvance {
        space_id: space_id()?,
        endpoint_id: endpoint,
        sequence: 3,
        issued_at_ms: 2,
        expires_at_ms: 20,
        record_hash: [8; 32],
        signed_record: b"stale".to_vec(),
    })?;
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
    let endpoint = endpoint_id()?;
    let first = AddressAdvance {
        space_id: space_id()?,
        endpoint_id: endpoint,
        sequence: 4,
        issued_at_ms: 10,
        expires_at_ms: 20,
        record_hash: [7; 32],
        signed_record: b"address-four".to_vec(),
    };
    let mut repository = Repository::open(&config)?;
    repository.create_space(&super::space_fixture::space_record()?)?;
    assert!(matches!(
        repository.advance_address(&first)?,
        AddressRecordOutcome::Advanced { .. }
    ));
    drop(repository);
    let mut repository = Repository::open(&config)?;

    // When
    let replay = repository.advance_address(&first)?;
    let fork = repository.advance_address(&AddressAdvance {
        record_hash: [8; 32],
        signed_record: b"fork-four".to_vec(),
        ..first.clone()
    })?;
    let rollback = repository.advance_address(&AddressAdvance {
        sequence: 3,
        record_hash: [9; 32],
        signed_record: b"rollback-three".to_vec(),
        ..first.clone()
    })?;

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
            .address_record(first.space_id, endpoint)?
            .ok_or("persisted address record was missing")?
            .signed_record(),
        b"address-four"
    );
    Ok(())
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
