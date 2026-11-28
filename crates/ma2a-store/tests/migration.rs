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
fn current_schema_is_idempotent_configured_and_lists_every_secret_column() -> TestResult {
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
        "hosted_services",
    ] {
        assert!(
            names.iter().any(|name| name == required),
            "missing table {required}"
        );
    }

    let key_columns = key_columns(&connection, &names)?;
    // Every column that names a key or a secret. Private keys appear only as references; the
    // rest are digests, a password verifier, and the Process ownership credential, which
    // docs/storage.md lists as what a backup carries.
    let expected = [
        ("consumed_invitation_tokens.token_hash", "BLOB"),
        ("endpoints.endpoint_key_ref", "TEXT"),
        ("invitations.token_hash", "BLOB"),
        ("processes.secret", "BLOB"),
        ("relay_configuration.private_key_path", "TEXT"),
        ("sessions.csrf_token_hash", "BLOB"),
        ("spaces.authority_key_ref", "TEXT"),
        ("ui_credentials.password_verifier", "BLOB"),
        ("ui_credentials.verifier_version", "INTEGER"),
    ];
    assert_eq!(
        key_columns,
        expected
            .into_iter()
            .map(|(column, kind)| (column.to_owned(), kind.to_owned()))
            .collect::<BTreeMap<_, _>>()
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
            if ["key", "secret", "token", "password", "verifier"]
                .iter()
                .any(|word| name.contains(word))
            {
                found.insert(format!("{table_name}.{name}"), column_type);
            }
        }
    }
    Ok(found)
}

#[test]
fn version_nine_exposures_migrate_unchanged_and_admit_hosted_services() -> TestResult {
    // Given a schema 9 store with an Agent exposure.
    let state = TempState::new("v9-exposures")?;
    let config = StoreConfig::new(state.path());
    let connection = Connection::open(config.database_path())?;
    for migration in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_web_auth.sql"),
        include_str!("../migrations/0003_relay_persistence.sql"),
        include_str!("../migrations/0004_local_mutation_replay.sql"),
        include_str!("../migrations/0005_relay_advertisement_activity.sql"),
        include_str!("../migrations/0006_mutation_fences.sql"),
        include_str!("../migrations/0007_service_exposures.sql"),
        include_str!("../migrations/0008_processes.sql"),
        include_str!("../migrations/0009_agents.sql"),
    ] {
        connection.execute_batch(migration)?;
    }
    connection.execute(
        "INSERT INTO service_exposures(space_id, kind, exposure_selector) VALUES (?1, 5, 'helper')",
        [[0x41_u8; 32].as_slice()],
    )?;
    assert!(
        connection
            .execute(
                "INSERT INTO service_exposures(space_id, kind, exposure_selector)
                 VALUES (?1, 6, 'example')",
                [[0x41_u8; 32].as_slice()],
            )
            .is_err(),
        "schema 9 refuses kind 6"
    );
    connection.pragma_update(None, "user_version", 9_u32)?;
    drop(connection);
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt as _};
        fs::set_permissions(config.database_path(), fs::Permissions::from_mode(0o600))?;
    }

    // When
    drop(Repository::open(&config)?);

    // Then the row survives and kind 6 is accepted.
    let connection = Connection::open(config.database_path())?;
    assert_eq!(
        connection.query_row(
            "SELECT exposure_selector FROM service_exposures WHERE kind = 5",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "helper"
    );
    connection.execute(
        "INSERT INTO service_exposures(space_id, kind, exposure_selector) VALUES (?1, 6, 'example')",
        [[0x41_u8; 32].as_slice()],
    )?;
    assert!(
        connection
            .execute(
                "INSERT INTO service_exposures(space_id, kind, exposure_selector) VALUES (?1, 6, '')",
                [[0x41_u8; 32].as_slice()],
            )
            .is_err(),
        "a service exposure names one service"
    );
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))?,
        SCHEMA_VERSION
    );
    Ok(())
}

#[test]
fn storage_doc_matches_current_schema_version_and_lists_every_migration() -> TestResult {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc = std::fs::read_to_string(manifest_dir.join("../../docs/storage.md"))?;
    let expected_heading = format!("## Schema Version {SCHEMA_VERSION}");
    assert!(
        doc.contains(&expected_heading),
        "docs/storage.md must contain `{expected_heading}`"
    );
    let expected_reject = format!("newer than schema version {SCHEMA_VERSION}");
    assert!(
        doc.contains(&expected_reject),
        "docs/storage.md must contain `{expected_reject}`"
    );
    let expected_range = format!("1 through {SCHEMA_VERSION} in order");
    assert!(
        doc.contains(&expected_range),
        "docs/storage.md must contain `{expected_range}`"
    );
    for entry in std::fs::read_dir(manifest_dir.join("migrations"))? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            assert!(
                doc.contains(&format!("`{name}`")),
                "docs/storage.md must list migration `{name}`"
            );
        }
    }
    Ok(())
}
