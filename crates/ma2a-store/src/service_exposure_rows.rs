//! Row decoding for persisted service exposures.

use ma2a_core::{
    SpaceId,
    service::{
        AgentId, ExportedRoot, ProxyHost, ProxyHostPattern, ProxyRule, RootAccess, RootName,
        ServiceName, SpaceExposureView,
    },
};

use rusqlite::OptionalExtension as _;

use crate::{ExposureRecord, FileRootExposure, LocalExposure, Repository, StoreError};

type ExposureRow = (
    (Vec<u8>, u8, String),
    Option<String>,
    Option<u8>,
    Option<String>,
    Option<u16>,
    Option<u16>,
    Option<String>,
);

pub(crate) fn load_exposures(
    connection: &rusqlite::Connection,
) -> Result<Vec<ExposureRecord>, StoreError> {
    let mut statement = connection.prepare(
        "SELECT e.space_id, e.kind, e.root_name, e.root_access, e.proxy_host,
                e.port_start, e.port_end, r.path, e.exposure_selector
         FROM service_exposures e LEFT JOIN file_roots r ON r.name = e.root_name
         ORDER BY e.space_id, e.kind, e.exposure_selector",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            (row.get(0)?, row.get(1)?, row.get(8)?),
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get(5)?,
            row.get(6)?,
            row.get(7)?,
        ))
    })?;
    rows.map(|row| decode_row(row?)).collect()
}

fn decode_row(row: ExposureRow) -> Result<ExposureRecord, StoreError> {
    const MALFORMED: StoreError = StoreError::SchemaMismatch {
        detail: "persisted service exposure is malformed",
    };
    let ((space_id, kind, selector), root_name, root_access, proxy_host, start, end, path) = row;
    let space_id = SpaceId::try_from(space_id.as_slice()).map_err(|_| MALFORMED)?;
    let exposure = match (kind, root_name, root_access, path, proxy_host, start, end) {
        (0, Some(name), Some(access), Some(path), None, None, None) => {
            let access = match access {
                0 => RootAccess::ReadOnly,
                1 => RootAccess::ReadWrite,
                _ => return Err(MALFORMED),
            };
            let name = RootName::new(&name).map_err(|_| MALFORMED)?;
            LocalExposure::FilesRoot(FileRootExposure::new(name, path, access))
        }
        (1, None, None, None, None, None, None) => LocalExposure::Exec,
        (2, None, None, None, None, None, None) => LocalExposure::Shell,
        (4, None, None, None, None, None, None) => LocalExposure::Process,
        (5, None, None, None, None, None, None) => {
            LocalExposure::Agent(AgentId::new(&selector).map_err(|_| MALFORMED)?)
        }
        (6, None, None, None, None, None, None) => {
            LocalExposure::Service(ServiceName::new(&selector).map_err(|_| MALFORMED)?)
        }
        (3, None, None, None, Some(host), Some(start), Some(end)) => {
            let pattern = if host == "*" {
                ProxyHostPattern::Any
            } else {
                ProxyHostPattern::Exact(ProxyHost::new(&host).map_err(|_| MALFORMED)?)
            };
            LocalExposure::Proxy(ProxyRule::new(pattern, (start, end)).map_err(|_| MALFORMED)?)
        }
        _ => return Err(MALFORMED),
    };
    Ok(ExposureRecord { space_id, exposure })
}

impl Repository {
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
                    current.with_root(ExportedRoot::new(root.name().clone(), root.access()))
                }
                LocalExposure::Exec => current.with_exec(),
                LocalExposure::Shell => current.with_shell(),
                LocalExposure::Proxy(rule) => current.with_proxy_rule(rule),
                LocalExposure::Process => current.with_process(),
                LocalExposure::Agent(agent) => current.with_agent(agent),
                LocalExposure::Service(service) => current.with_service(service),
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
