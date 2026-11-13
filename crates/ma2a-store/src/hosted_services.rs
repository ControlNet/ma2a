//! Operator-registered hosted services.
//!
//! A registration changes only while its service is stopped. Links and calls are never
//! durable: after any restart, a process that serves a started service attaches again.

use ma2a_core::service::{ServiceManifest, ServiceName};
use rusqlite::{OptionalExtension as _, Row, params};

use crate::{
    Committed, Repository, StoreError, repository::increment_revision,
    service_exposures::current_revision,
};

const COLUMNS: &str = "service, manifest, enabled, created_at_ms, updated_at_ms";
const MALFORMED: StoreError = StoreError::SchemaMismatch {
    detail: "persisted hosted service is malformed",
};

/// One registered hosted service.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedServiceRecord {
    /// The service's name.
    pub name: ServiceName,
    /// What the service offers.
    pub manifest: ServiceManifest,
    /// Whether the operator has the service started.
    pub enabled: bool,
    /// When the service was first registered.
    pub created_at_ms: u64,
    /// When the registration or its started state last changed.
    pub updated_at_ms: u64,
}

/// What registering or unregistering a service did.
#[expect(
    clippy::exhaustive_enums,
    reason = "every registration outcome must be answered explicitly"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServiceRegistration {
    /// The registration is stored or removed; `true` when the service is new.
    Registered(Box<HostedServiceRecord>, bool),
    /// The service is started; stop it before changing or removing it.
    Active,
    /// No such service is registered.
    Missing,
}

fn read(row: &Row<'_>) -> rusqlite::Result<Result<HostedServiceRecord, StoreError>> {
    let name: String = row.get(0)?;
    let manifest: Vec<u8> = row.get(1)?;
    let (enabled, created_at_ms, updated_at_ms) = (row.get(2)?, row.get(3)?, row.get(4)?);
    Ok(
        match (
            ServiceName::new(&name),
            ServiceManifest::from_bytes(&manifest),
        ) {
            (Ok(name), Ok(manifest)) => Ok(HostedServiceRecord {
                name,
                manifest,
                enabled,
                created_at_ms,
                updated_at_ms,
            }),
            _ => Err(MALFORMED),
        },
    )
}

fn load(
    connection: &rusqlite::Connection,
    name: &ServiceName,
) -> Result<Option<HostedServiceRecord>, StoreError> {
    connection
        .query_row(
            &format!("SELECT {COLUMNS} FROM hosted_services WHERE service = ?1"),
            [name.as_str()],
            read,
        )
        .optional()?
        .transpose()
}

impl Repository {
    /// Registers a new service, or replaces the registration of a stopped one.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn register_service(
        &mut self,
        (name, manifest): (&ServiceName, &ServiceManifest),
        at_ms: u64,
    ) -> Result<Committed<ServiceRegistration>, StoreError> {
        let encoded = manifest.to_bytes()?;
        let transaction = self.immediate()?;
        let existing = load(&transaction, name)?;
        if existing.as_ref().is_some_and(|record| record.enabled) {
            let revision = current_revision(&transaction)?;
            transaction.commit()?;
            return Ok(Committed::new(revision, ServiceRegistration::Active));
        }
        transaction.execute(
            "INSERT INTO hosted_services (service, manifest, max_calls, enabled, created_at_ms,
             updated_at_ms) VALUES (?1, ?2, ?3, 0, ?4, ?4)
             ON CONFLICT (service) DO UPDATE SET manifest = ?2, max_calls = ?3,
             updated_at_ms = ?4",
            params![name.as_str(), encoded, manifest.max_calls(), at_ms],
        )?;
        let revision = increment_revision(&transaction)?;
        let record = load(&transaction, name)?.ok_or(StoreError::SchemaMismatch {
            detail: "a registered service could not be read back",
        })?;
        transaction.commit()?;
        Ok(Committed::new(
            revision,
            ServiceRegistration::Registered(Box::new(record), existing.is_none()),
        ))
    }

    /// Removes a stopped service's registration and, in the same commit, every exposure that
    /// names it: a service registered later under that name may be another program, and
    /// consent given to this one must not carry over. Replacing a registration keeps them.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn unregister_service(
        &mut self,
        name: &ServiceName,
    ) -> Result<Committed<ServiceRegistration>, StoreError> {
        let transaction = self.immediate()?;
        let outcome = match load(&transaction, name)? {
            None => ServiceRegistration::Missing,
            Some(record) if record.enabled => ServiceRegistration::Active,
            Some(record) => {
                transaction.execute(
                    "DELETE FROM hosted_services WHERE service = ?1",
                    [name.as_str()],
                )?;
                transaction.execute(
                    "DELETE FROM service_exposures WHERE kind = 6 AND exposure_selector = ?1",
                    [name.as_str()],
                )?;
                ServiceRegistration::Registered(Box::new(record), false)
            }
        };
        let revision = if matches!(outcome, ServiceRegistration::Registered(..)) {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, outcome))
    }

    /// Starts or stops a service; `None` when it is not registered.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn set_service_enabled(
        &mut self,
        name: &ServiceName,
        (enabled, at_ms): (bool, u64),
    ) -> Result<Committed<Option<HostedServiceRecord>>, StoreError> {
        let transaction = self.immediate()?;
        let changed = transaction.execute(
            "UPDATE hosted_services SET enabled = ?2, updated_at_ms = ?3
             WHERE service = ?1 AND enabled != ?2",
            params![name.as_str(), enabled, at_ms],
        )? > 0;
        let revision = if changed {
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        let record = load(&transaction, name)?;
        transaction.commit()?;
        Ok(Committed::new(revision, record))
    }

    /// Returns one registration.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a persisted row is malformed.
    pub fn hosted_service(
        &self,
        name: &ServiceName,
    ) -> Result<Option<HostedServiceRecord>, StoreError> {
        load(&self.connection, name)
    }

    /// Returns at most `limit` registrations after `after`, by name.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a persisted row is malformed.
    pub fn hosted_services(
        &self,
        (after, limit): (Option<&ServiceName>, u32),
    ) -> Result<Committed<Vec<HostedServiceRecord>>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = current_revision(&transaction)?;
        let records = {
            let mut statement = transaction.prepare(&format!(
                "SELECT {COLUMNS} FROM hosted_services WHERE ?1 IS NULL OR service > ?1
                 ORDER BY service LIMIT ?2"
            ))?;
            let rows = statement.query_map(params![after.map(ServiceName::as_str), limit], read)?;
            rows.map(|row| row?).collect::<Result<Vec<_>, _>>()?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, records))
    }
}
