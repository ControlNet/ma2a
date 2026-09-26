//! Schema migration and structural secret-column coverage.

#[path = "common/support.rs"]
mod support;

use std::collections::BTreeMap;

use ma2a_core::{
    MemberCapabilities, SpaceAuthoritySecret, SpaceGenesisIdentity, SpaceGenesisOwner,
    SpaceGenesisV1, SpaceMemberV1, SpacePolicyV1,
};
use ma2a_store::{Repository, SCHEMA_VERSION, StoreConfig, StoreError, derive_password_verifier};
use rusqlite::Connection;
use support::{TempState, TestResult};

#[test]
fn current_schema_is_idempotent_configured_and_contains_only_key_references() -> TestResult {
    // Given
    let state = TempState::new("migration")?;
    let config = StoreConfig::new(state.path());

    // When
    let repository = Repository::open(&config)?;
    let settings = repository.database_settings()?;
    assert_eq!(settings.journal_mode, "wal");
    assert!(settings.foreign_keys);
    assert_eq!(settings.busy_timeout_ms, 5_000);
    drop(repository);
    drop(Repository::open(&config)?);

    // Then
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?,
        SCHEMA_VERSION
    );
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| row
            .get::<_, u32>(0))?,
        SCHEMA_VERSION
    );

    let mut tables = connection.prepare(
        "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names = tables
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for required in [
        "runtime_metadata",
        "endpoints",
        "spaces",
        "manifests",
        "members",
        "member_revocations",
        "invitations",
        "consumed_invitation_tokens",
        "address_state",
        "relay_advertisement_state",
        "relay_configuration",
        "relay_public_fallback_urls",
        "relay_served_spaces",
        "relay_publication_state",
        "relay_observations",
        "local_mutation_replay",
        "ui_credentials",
        "sessions",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing table {required}"
        );
    }

    let key_columns = key_columns(&connection, &names)?;
    assert_eq!(
        key_columns,
        BTreeMap::from([
            ("endpoints.endpoint_key_ref".to_owned(), "TEXT".to_owned()),
            (
                "relay_configuration.private_key_path".to_owned(),
                "TEXT".to_owned()
            ),
            ("spaces.authority_key_ref".to_owned(), "TEXT".to_owned()),
        ])
    );
    Ok(())
}

#[test]
fn future_schema_is_rejected_without_switching_journal_mode() -> TestResult {
    // Given
    let state = TempState::new("future-schema")?;
    let config = StoreConfig::new(state.path());
    let connection = Connection::open(config.database_path())?;
    connection.pragma_update(None, "user_version", SCHEMA_VERSION + 1)?;
    drop(connection);
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt as _};
        fs::set_permissions(config.database_path(), fs::Permissions::from_mode(0o600))?;
    }

    // When
    let result = Repository::open(&config);

    // Then
    assert!(matches!(
        result,
        Err(StoreError::FutureSchema { found, supported })
            if found == SCHEMA_VERSION + 1 && supported == SCHEMA_VERSION
    ));
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?,
        "delete"
    );
    Ok(())
}

#[test]
fn version_one_credentials_migrate_to_a_valid_session_epoch() -> TestResult {
    // Given
    let state = TempState::new("v1-web-auth")?;
    let config = StoreConfig::new(state.path());
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch(include_str!("../migrations/0001_init.sql"))?;
    connection.pragma_update(None, "user_version", 1_u32)?;
    let verifier = derive_password_verifier(b"legacy-test-passphrase-9!")?;
    connection.execute(
        "INSERT INTO ui_credentials(singleton, password_verifier, verifier_version, updated_at_ms)
         VALUES (1, ?1, 1, 10)",
        [verifier.as_slice()],
    )?;
    drop(connection);
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt as _};
        fs::set_permissions(config.database_path(), fs::Permissions::from_mode(0o600))?;
    }

    // When
    let repository = Repository::open(&config)?;
    let credential = repository
        .credential()?
        .ok_or("missing migrated credential")?;

    // Then
    assert_eq!(credential.auth_epoch(), 1);
    assert_eq!(credential.verifier(), verifier);
    Ok(())
}

#[test]
fn version_four_relay_rows_migrate_as_active_high_water() -> TestResult {
    // Given
    let state = TempState::new("v4-relay-activity")?;
    let config = StoreConfig::new(state.path());
    let connection = Connection::open(config.database_path())?;
    connection.execute_batch(include_str!("../migrations/0001_init.sql"))?;
    connection.execute_batch(include_str!("../migrations/0002_web_auth.sql"))?;
    connection.execute_batch(include_str!("../migrations/0003_relay_persistence.sql"))?;
    connection.execute_batch(include_str!("../migrations/0004_local_mutation_replay.sql"))?;
    let authority = SpaceAuthoritySecret::from_bytes([0x31; 32]);
    let provider = iroh_base::SecretKey::from_bytes(&[0x32; 32]);
    let genesis = SpaceGenesisV1::new(
        SpaceGenesisIdentity::new([0x33; 32], 1, authority.public_key())?,
        SpaceGenesisOwner::new(
            SpaceMemberV1::new(
                provider.public().into(),
                "relay-provider".to_owned(),
                MemberCapabilities::new(true, true),
            )?,
            SpacePolicyV1::phase_one_default(),
        ),
    )
    .sign(&authority)?;
    connection.execute(
        "INSERT INTO spaces(
            space_id, genesis_cbor, latest_manifest_generation, latest_manifest_hash
         ) VALUES (?1, ?2, 0, ?3)",
        (
            genesis.space_id().as_bytes().as_slice(),
            genesis.canonical_bytes(),
            genesis.chain_hash().as_slice(),
        ),
    )?;
    connection.execute(
        "INSERT INTO manifests(
            space_id, generation, previous_hash, manifest_hash, signed_manifest
         ) VALUES (?1, 0, NULL, ?2, ?3)",
        (
            genesis.space_id().as_bytes().as_slice(),
            genesis.chain_hash().as_slice(),
            genesis.canonical_bytes(),
        ),
    )?;
    connection.execute(
        "INSERT INTO members(space_id, endpoint_id, role, accepted_generation)
         VALUES (?1, ?2, 0, 0)",
        (
            genesis.space_id().as_bytes().as_slice(),
            provider.public().as_bytes().as_slice(),
        ),
    )?;
    connection.execute(
        "INSERT INTO relay_advertisement_state(
            space_id, relay_endpoint_id, sequence, issued_at_ms, expires_at_ms,
            advertisement_hash, signed_advertisement
        ) VALUES (?1, ?2, 7, 10, 20, ?3, ?4)",
        (
            genesis.space_id().as_bytes().as_slice(),
            provider.public().as_bytes().as_slice(),
            [0x33; 32].as_slice(),
            [0x34].as_slice(),
        ),
    )?;
    connection.pragma_update(None, "user_version", 4_u32)?;
    drop(connection);
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt as _};
        fs::set_permissions(config.database_path(), fs::Permissions::from_mode(0o600))?;
    }

    // When
    drop(Repository::open(&config)?);

    // Then
    let connection = Connection::open(config.database_path())?;
    assert!(
        connection.query_row("SELECT active FROM relay_advertisement_state", [], |row| {
            row.get::<_, bool>(0)
        })?
    );
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?,
        SCHEMA_VERSION
    );
    assert_eq!(
        connection.query_row(
            "SELECT name FROM schema_migrations WHERE version = 5",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "relay_advertisement_activity"
    );
    Ok(())
}

fn key_columns(
    connection: &Connection,
    table_names: &[String],
) -> Result<BTreeMap<String, String>, rusqlite::Error> {
    let mut found = BTreeMap::new();
    for table_name in table_names {
        let sql = format!("PRAGMA table_info('{table_name}')");
        let mut statement = connection.prepare(&sql)?;
        for column in statement.query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })? {
            let (name, column_type) = column?;
            if name.contains("key") {
                found.insert(format!("{table_name}.{name}"), column_type);
            }
        }
    }
    Ok(found)
}
