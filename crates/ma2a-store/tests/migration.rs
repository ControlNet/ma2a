//! Schema migration and structural secret-column coverage.

#[path = "common/support.rs"]
mod support;

use std::collections::BTreeMap;

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
        "relay_observations",
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
