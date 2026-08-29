use ma2a_store::{
    AddressAdvance, PasswordReset, RelayAdvertisementAdvance, RelayConfiguration, RelayObservation,
    Repository, RuntimeMetadataUpdate, SequenceOutcome, SessionRecord, StoreConfig,
};
use rusqlite::Connection;

use super::{endpoint_id, space_id, support::TempState};
use crate::support::TestResult;

#[test]
fn address_and_advertisement_reject_stale_sequences() -> TestResult {
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
    let advertisement = repository.advance_relay_advertisement(&RelayAdvertisementAdvance {
        space_id: space_id()?,
        relay_endpoint_id: endpoint,
        sequence: 9,
        expires_at_ms: 30,
        advertisement_hash: [9; 32],
        signed_advertisement: b"relay".to_vec(),
    })?;

    // Then
    assert!(matches!(address, SequenceOutcome::Advanced { .. }));
    assert_eq!(
        stale_address,
        SequenceOutcome::Stale {
            current_sequence: 4
        }
    );
    assert!(matches!(advertisement, SequenceOutcome::Advanced { .. }));
    Ok(())
}

#[test]
fn password_reset_revokes_existing_sessions_in_one_revision() -> TestResult {
    // Given
    let state = TempState::new("password-reset")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    repository.create_session(&SessionRecord {
        session_id_hash: [10; 32],
        created_at_ms: 1,
        expires_at_ms: 100,
    })?;

    // When
    repository.reset_password(&PasswordReset {
        verifier: b"argon2id verifier".to_vec(),
        verifier_version: 1,
        now_ms: 50,
    })?;
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
    assert_eq!(
        connection.query_row("SELECT password_verifier FROM ui_credentials", [], |row| {
            row.get::<_, Vec<u8>>(0)
        })?,
        b"argon2id verifier"
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
    repository.set_relay_configuration(&RelayConfiguration {
        public_fallback_enabled: true,
        public_relay_url: Some("https://relay.invalid".to_owned()),
        private_provider_enabled: false,
        listener_address: None,
        tls_mode: None,
    })?;
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
