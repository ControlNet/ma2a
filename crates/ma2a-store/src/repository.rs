use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction};

use crate::{
    DatabaseSettings, EndpointRecord, KeyKind, KeyReference, KeyStore, SpaceRecord, StoreConfig,
    StoreError, migrations, permissions,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Synchronous `SQLite` repository intended for a dedicated blocking owner.
pub struct Repository {
    pub(crate) connection: Connection,
    key_store: KeyStore,
}

impl Repository {
    /// Opens, migrates, and fail-closed validates one current-user store.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when filesystem protections, database setup,
    /// migration, schema validation, or protected-key references are invalid.
    pub fn open(config: &StoreConfig) -> Result<Self, StoreError> {
        let key_store = KeyStore::open(config.state_dir())?;
        let database_path = config.database_path();
        if database_path.exists() {
            permissions::validate_private_file(&database_path, "SQLite database")?;
        }
        let mut connection = Connection::open_with_flags(
            &database_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if !database_path.exists() {
            return Err(StoreError::SchemaMismatch {
                detail: "SQLite did not create the database file",
            });
        }
        permissions::protect_new_file(&database_path, "SQLite database")?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let version = migrations::current_version(&connection)?;
        if version > migrations::SCHEMA_VERSION {
            return Err(StoreError::FutureSchema {
                found: version,
                supported: migrations::SCHEMA_VERSION,
            });
        }
        let journal_mode =
            connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| {
                row.get::<_, String>(0)
            })?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::SchemaMismatch {
                detail: "SQLite refused WAL journal mode",
            });
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        migrations::migrate(&mut connection)?;
        let repository = Self {
            connection,
            key_store,
        };
        repository.validate_key_references()?;
        Ok(repository)
    }

    /// Returns the current monotonic Runtime state revision.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot read runtime metadata.
    pub fn revision(&self) -> Result<u64, StoreError> {
        Ok(self.connection.query_row(
            "SELECT revision FROM runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Reads the effective safety settings from this `SQLite` connection.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot read a connection setting.
    pub fn database_settings(&self) -> Result<DatabaseSettings, StoreError> {
        Ok(DatabaseSettings {
            journal_mode: self
                .connection
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))?,
            foreign_keys: self
                .connection
                .query_row("PRAGMA foreign_keys", [], |row| row.get(0))?,
            busy_timeout_ms: self
                .connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get(0))?,
        })
    }

    /// Stores the one Runtime Endpoint identity and protected-key reference atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the protected key is unavailable or the
    /// transaction cannot be committed.
    pub fn set_endpoint(&mut self, endpoint: &EndpointRecord) -> Result<u64, StoreError> {
        self.key_store
            .validate_reference(KeyKind::Endpoint, &endpoint.key_reference)?;
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO endpoints(singleton, endpoint_id, endpoint_key_ref) VALUES (1, ?1, ?2)
             ON CONFLICT(singleton) DO UPDATE SET endpoint_id = excluded.endpoint_id,
             endpoint_key_ref = excluded.endpoint_key_ref",
            (
                endpoint.endpoint_id.as_bytes().as_slice(),
                endpoint.key_reference.as_str(),
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Stores Space genesis and an optional protected authority-key reference atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the protected authority key is unavailable or
    /// the transaction cannot be committed.
    pub fn create_space(&mut self, space: &SpaceRecord) -> Result<u64, StoreError> {
        if let Some(reference) = &space.authority_key_reference {
            self.key_store
                .validate_reference(KeyKind::SpaceAuthority, reference)?;
        }
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO spaces(space_id, genesis_cbor, authority_key_ref) VALUES (?1, ?2, ?3)",
            (
                space.space_id.as_bytes().as_slice(),
                space.genesis_cbor.as_slice(),
                space
                    .authority_key_reference
                    .as_ref()
                    .map(KeyReference::as_str),
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    pub(crate) fn immediate(&mut self) -> Result<Transaction<'_>, StoreError> {
        Ok(self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?)
    }

    fn validate_key_references(&self) -> Result<(), StoreError> {
        let endpoint_reference = self
            .connection
            .query_row(
                "SELECT endpoint_key_ref FROM endpoints WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(value) = endpoint_reference {
            self.key_store
                .validate_reference(KeyKind::Endpoint, &KeyReference::parse(&value)?)?;
        }
        let mut statement = self
            .connection
            .prepare("SELECT authority_key_ref FROM spaces WHERE authority_key_ref IS NOT NULL")?;
        let references = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for value in references {
            self.key_store
                .validate_reference(KeyKind::SpaceAuthority, &KeyReference::parse(&value)?)?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Repository {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Repository { synchronous: true }")
    }
}

pub(crate) fn increment_revision(transaction: &Transaction<'_>) -> Result<u64, StoreError> {
    transaction.execute(
        "UPDATE runtime_metadata SET revision = revision + 1 WHERE singleton = 1",
        [],
    )?;
    Ok(transaction.query_row(
        "SELECT revision FROM runtime_metadata WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?)
}

use rusqlite::OptionalExtension as _;
