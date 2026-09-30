//! Durable per-Space local service exposure and upload staging records.

use ma2a_core::{
    ProtocolError, SpaceId,
    service::{ExportedRoot, ProxyHostPattern, ProxyRule, RootAccess, RootName, SpaceExposureView},
};
use rusqlite::{OptionalExtension as _, params};

use crate::{
    Committed, Repository, StoreError, repository::increment_revision,
    service_exposure_rows::load_exposures,
};

/// One exported file root: a Runtime-wide name, its local directory, and per-Space access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRootExposure {
    name: RootName,
    path: String,
    access: RootAccess,
}

impl FileRootExposure {
    /// Creates a root exposure. `path` must already be an absolute canonical directory.
    pub const fn new(name: RootName, path: String, access: RootAccess) -> Self {
        Self { name, path, access }
    }

    /// Returns the root name.
    pub const fn name(&self) -> &RootName {
        &self.name
    }

    /// Returns the local canonical directory.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the access mode in this Space.
    pub const fn access(&self) -> RootAccess {
        self.access
    }
}

/// One thing this Runtime serves inside one Space.
#[expect(
    clippy::exhaustive_enums,
    reason = "exposure kinds are a closed persisted vocabulary"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalExposure {
    /// An exported file root.
    FilesRoot(FileRootExposure),
    /// One-shot Exec.
    Exec,
    /// Interactive Shell.
    Shell,
    /// One allowed proxy destination rule.
    Proxy(ProxyRule),
}

impl LocalExposure {
    const fn kind(&self) -> u8 {
        match self {
            Self::FilesRoot(_) => 0,
            Self::Exec => 1,
            Self::Shell => 2,
            Self::Proxy(_) => 3,
        }
    }

    /// Returns the stable key identifying this exposure within its Space and kind.
    pub fn key(&self) -> String {
        match self {
            Self::FilesRoot(root) => root.name.as_str().to_owned(),
            Self::Exec | Self::Shell => String::new(),
            Self::Proxy(rule) => {
                let host = match rule.host() {
                    ProxyHostPattern::Any => "*",
                    ProxyHostPattern::Exact(host) => host.as_str(),
                };
                let (start, end) = rule.ports();
                format!("{host}:{start}-{end}")
            }
        }
    }
}

/// One persisted exposure and the Space it belongs to.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExposureRecord {
    /// The Space this exposure serves.
    pub space_id: SpaceId,
    /// What is served.
    pub exposure: LocalExposure,
}

impl Repository {
    /// Adds or updates one exposure for a Space this Runtime holds.
    ///
    /// # Errors
    /// Returns [`StoreError::SpaceNotFound`] for an unknown Space and a conflict when a root
    /// name is already bound to a different local directory.
    pub fn expose_service(
        &mut self,
        space_id: SpaceId,
        exposure: &LocalExposure,
    ) -> Result<Committed<()>, StoreError> {
        let transaction = self.immediate()?;
        let space_exists = transaction
            .query_row(
                "SELECT 1 FROM spaces WHERE space_id = ?1",
                [space_id.as_bytes().as_slice()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !space_exists {
            return Err(StoreError::SpaceNotFound);
        }
        let (root_name, root_access, proxy) = match exposure {
            LocalExposure::FilesRoot(root) => {
                bind_root(&transaction, root)?;
                (
                    Some(root.name.as_str()),
                    Some(access_code(root.access)),
                    None,
                )
            }
            LocalExposure::Exec | LocalExposure::Shell => (None, None, None),
            LocalExposure::Proxy(rule) => (None, None, Some(rule)),
        };
        let proxy_host = proxy.map(|rule| match rule.host() {
            ProxyHostPattern::Any => "*".to_owned(),
            ProxyHostPattern::Exact(host) => host.as_str().to_owned(),
        });
        transaction.execute(
            "INSERT OR REPLACE INTO service_exposures
             (space_id, kind, exposure_selector, root_name, root_access, proxy_host, port_start, port_end)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                space_id.as_bytes().as_slice(),
                exposure.kind(),
                exposure.key(),
                root_name,
                root_access,
                proxy_host,
                proxy.map(|rule| rule.ports().0),
                proxy.map(|rule| rule.ports().1),
            ],
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(Committed::new(revision, ()))
    }

    /// Removes one exposure; an unreferenced file root is forgotten with its last exposure.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the change cannot be committed.
    pub fn unexpose_service(
        &mut self,
        space_id: SpaceId,
        exposure: &LocalExposure,
    ) -> Result<Committed<bool>, StoreError> {
        let transaction = self.immediate()?;
        let removed = transaction.execute(
            "DELETE FROM service_exposures WHERE space_id = ?1 AND kind = ?2 AND exposure_selector = ?3",
            params![
                space_id.as_bytes().as_slice(),
                exposure.kind(),
                exposure.key()
            ],
        )? > 0;
        let revision = if removed {
            forget_unbound_roots(&transaction)?;
            increment_revision(&transaction)?
        } else {
            current_revision(&transaction)?
        };
        transaction.commit()?;
        Ok(Committed::new(revision, removed))
    }

    /// Lists every exposure in canonical order with the revision of the same read.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a persisted row is malformed.
    pub fn service_exposures(&self) -> Result<Committed<Vec<ExposureRecord>>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = current_revision(&transaction)?;
        let records = load_exposures(&transaction)?;
        transaction.commit()?;
        Ok(Committed::new(revision, records))
    }

    /// Returns the authorization view of every exposure, grouped by Space.
    ///
    /// # Errors
    /// Returns [`StoreError`] when a persisted row is malformed.
    pub fn service_exposure_views(&self) -> Result<Vec<SpaceExposureView>, StoreError> {
        let mut views: Vec<SpaceExposureView> = Vec::new();
        for record in load_exposures(&self.connection)? {
            let index = if let Some(index) = views
                .iter()
                .position(|view| view.space_id() == Some(record.space_id))
            {
                index
            } else {
                views.push(SpaceExposureView::new(record.space_id));
                views.len() - 1
            };
            let Some(view) = views.get_mut(index) else {
                continue;
            };
            let current = std::mem::take(view);
            *view = match record.exposure {
                LocalExposure::FilesRoot(root) => {
                    current.with_root(ExportedRoot::new(root.name, root.access))
                }
                LocalExposure::Exec => current.with_exec(),
                LocalExposure::Shell => current.with_shell(),
                LocalExposure::Proxy(rule) => current.with_proxy_rule(rule),
            };
        }
        Ok(views)
    }

    /// Returns the local directory bound to an exported root name, if any.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the row cannot be read.
    pub fn file_root_path(&self, name: &RootName) -> Result<Option<String>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT path FROM file_roots WHERE name = ?1",
                [name.as_str()],
                |row| row.get(0),
            )
            .optional()?)
    }
}

fn bind_root(
    transaction: &rusqlite::Transaction<'_>,
    root: &FileRootExposure,
) -> Result<(), StoreError> {
    let existing: Option<String> = transaction
        .query_row(
            "SELECT path FROM file_roots WHERE name = ?1",
            [root.name.as_str()],
            |row| row.get(0),
        )
        .optional()?;
    match existing {
        Some(path) if path != root.path => Err(StoreError::Protocol(ProtocolError::CONFLICT)),
        Some(_) => Ok(()),
        None => {
            transaction.execute(
                "INSERT INTO file_roots(name, path) VALUES (?1, ?2)",
                params![root.name.as_str(), root.path],
            )?;
            Ok(())
        }
    }
}

/// Withdraws every exposure of a Space whose stored chain no longer lists this Endpoint, and
/// forgets file roots no exposure binds any more.
///
/// Consent to serve a Space lasts only while this Endpoint belongs to it; a later rejoin
/// needs fresh consent. Nothing is withdrawn before this Runtime has an identity.
pub(crate) fn withdraw_departed(
    transaction: &rusqlite::Transaction<'_>,
    space_id: SpaceId,
) -> Result<(), StoreError> {
    let withdrawn = transaction.execute(
        "DELETE FROM service_exposures WHERE space_id = ?1
         AND EXISTS (SELECT 1 FROM endpoints)
         AND NOT EXISTS (SELECT 1 FROM members JOIN endpoints
             ON members.endpoint_id = endpoints.endpoint_id WHERE members.space_id = ?1)",
        [space_id.as_bytes().as_slice()],
    )?;
    if withdrawn > 0 {
        forget_unbound_roots(transaction)?;
    }
    Ok(())
}

fn forget_unbound_roots(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute(
        "DELETE FROM file_roots WHERE name NOT IN
         (SELECT root_name FROM service_exposures WHERE root_name IS NOT NULL)",
        [],
    )?;
    Ok(())
}

const fn access_code(access: RootAccess) -> u8 {
    match access {
        RootAccess::ReadOnly => 0,
        RootAccess::ReadWrite => 1,
    }
}

pub(crate) fn current_revision(connection: &rusqlite::Connection) -> Result<u64, StoreError> {
    Ok(connection.query_row(
        "SELECT revision FROM runtime_metadata WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?)
}
