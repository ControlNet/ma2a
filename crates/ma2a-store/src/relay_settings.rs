use ma2a_core::SpaceId;

use crate::repository::increment_revision;
use crate::{RelayConfiguration, RelayTransportConfiguration, Repository, StoreError};

impl Repository {
    /// Loads the desired public/private relay configuration.
    ///
    /// # Errors
    /// Returns [`StoreError`] when relay configuration is malformed or cannot be read.
    pub fn relay_configuration(&self) -> Result<RelayConfiguration, StoreError> {
        Ok(self.relay_configuration_committed()?.into_value())
    }

    /// Loads desired relay settings and revision from one read transaction.
    ///
    /// # Errors
    /// Returns an error when the coherent configuration cannot be read.
    pub fn relay_configuration_committed(
        &self,
    ) -> Result<crate::Committed<RelayConfiguration>, StoreError> {
        let transaction = self.connection.unchecked_transaction()?;
        let revision = transaction.query_row(
            "SELECT revision FROM runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?;
        let configuration = load_configuration(&transaction)?;
        transaction.commit()?;
        Ok(crate::Committed::new(revision, configuration))
    }

    /// Reserves and persists the next provider-owned advertisement sequence.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the sequence or revision cannot be committed.
    pub fn reserve_private_relay_sequence(&mut self) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        let current = transaction.query_row(
            "SELECT sequence FROM relay_publication_state WHERE singleton = 1",
            [],
            |row| row.get::<_, u64>(0),
        )?;
        let sequence = current.checked_add(1).ok_or(StoreError::SchemaMismatch {
            detail: "private relay publication sequence exhausted",
        })?;
        transaction.execute(
            "UPDATE relay_publication_state SET sequence = ?1 WHERE singleton = 1",
            [sequence],
        )?;
        increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(sequence)
    }
}

pub(crate) fn load_configuration(
    connection: &rusqlite::Connection,
) -> Result<RelayConfiguration, StoreError> {
    let (
        public_fallback_enabled,
        private_provider_enabled,
        listener_address,
        private_relay_url,
        tls_mode,
        certificate_path,
        private_key_path,
    ) = connection.query_row(
        "SELECT public_fallback_enabled, private_provider_enabled, listener_address,
             private_relay_url, tls_mode, certificate_path, private_key_path
             FROM relay_configuration WHERE singleton = 1",
        [],
        |row| {
            Ok((
                row.get::<_, bool>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<u8>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        },
    )?;
    let mut public_urls =
        connection.prepare("SELECT relay_url FROM relay_public_fallback_urls ORDER BY position")?;
    let public_relay_urls = public_urls
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut served =
        connection.prepare("SELECT space_id FROM relay_served_spaces ORDER BY space_id")?;
    let served_spaces = served
        .query_map([], |row| row.get::<_, Vec<u8>>(0))?
        .map(|row| {
            let bytes = row?;
            SpaceId::try_from(bytes.as_slice()).map_err(|_| StoreError::SchemaMismatch {
                detail: "persisted relay served Space has invalid length",
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let transport = match (tls_mode, certificate_path, private_key_path) {
        (None, None, None) => None,
        (Some(0), Some(certificate_path), Some(private_key_path)) => {
            Some(RelayTransportConfiguration::NativeTls {
                certificate_path,
                private_key_path,
            })
        }
        (Some(1), None, None) => Some(RelayTransportConfiguration::ExternalTlsTermination),
        _ => {
            return Err(StoreError::SchemaMismatch {
                detail: "persisted relay transport configuration is inconsistent",
            });
        }
    };
    Ok(RelayConfiguration {
        public_fallback_enabled,
        public_relay_urls,
        private_provider_enabled,
        listener_address,
        private_relay_url,
        served_spaces,
        transport,
    })
}
