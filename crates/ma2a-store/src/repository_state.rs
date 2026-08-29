use crate::{
    EndpointObservationUpdate, RelayConfiguration, RelayObservation, Repository, RuntimeMetadata,
    RuntimeMetadataUpdate, StoreError, repository::increment_revision,
};

impl Repository {
    /// Loads the stable local UDP port used by the Runtime Endpoint.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the persisted port is invalid or inaccessible.
    pub fn endpoint_bind_port(&self) -> Result<Option<u16>, StoreError> {
        Ok(self.connection.query_row(
            "SELECT endpoint_bind_port FROM runtime_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )?)
    }

    /// Persists the stable local UDP port used by the Runtime Endpoint.
    ///
    /// # Errors
    /// Returns [`StoreError`] when the port cannot be committed.
    pub fn set_endpoint_bind_port(&mut self, port: u16) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "UPDATE runtime_metadata SET endpoint_bind_port = ?1 WHERE singleton = 1",
            [port],
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Loads persisted Runtime lifecycle and Endpoint observation state.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when runtime metadata is malformed or cannot be read.
    pub fn runtime_metadata(&self) -> Result<RuntimeMetadata, StoreError> {
        type MetadataRow = (
            u64,
            Option<Vec<u8>>,
            Option<bool>,
            Option<i64>,
            Option<i64>,
            Option<bool>,
            Option<u64>,
            Option<u64>,
            Option<u64>,
        );
        let row: MetadataRow = self.connection.query_row(
            "SELECT revision, boot_id, last_shutdown_clean, last_shutdown_at_ms,
             endpoint_observed_at_ms, endpoint_ready, direct_address_count,
             relay_address_count, membership_count FROM runtime_metadata WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )?;
        let boot_id = row
            .1
            .map(|bytes| {
                <[u8; 16]>::try_from(bytes).map_err(|_| StoreError::SchemaMismatch {
                    detail: "persisted Runtime boot identifier has invalid length",
                })
            })
            .transpose()?;
        let endpoint_observation = match (row.4, row.5, row.6, row.7, row.8) {
            (Some(observed_at_ms), Some(ready), Some(direct), Some(relay), Some(memberships)) => {
                Some(EndpointObservationUpdate {
                    observed_at_ms,
                    ready,
                    direct_address_count: direct,
                    relay_address_count: relay,
                    membership_count: memberships,
                })
            }
            (None, None, None, None, None) => None,
            _ => {
                return Err(StoreError::SchemaMismatch {
                    detail: "persisted Endpoint observation is incomplete",
                });
            }
        };
        Ok(RuntimeMetadata {
            revision: row.0,
            boot_id,
            last_shutdown_clean: row.2,
            last_shutdown_at_ms: row.3,
            endpoint_observation,
        })
    }

    /// Stores boot/shutdown metadata and advances revision atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when runtime metadata cannot be committed.
    pub fn record_runtime_metadata(
        &mut self,
        metadata: &RuntimeMetadataUpdate,
    ) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "UPDATE runtime_metadata SET boot_id = ?1, last_shutdown_clean = ?2,
             last_shutdown_at_ms = ?3 WHERE singleton = 1",
            (
                metadata.boot_id.as_slice(),
                metadata.last_shutdown_clean,
                metadata.observed_at_ms,
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Stores the latest Endpoint observation and advances revision atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when the observation cannot be committed.
    pub fn record_endpoint_observation(
        &mut self,
        observation: &EndpointObservationUpdate,
    ) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "UPDATE runtime_metadata SET endpoint_observed_at_ms = ?1, endpoint_ready = ?2,
             direct_address_count = ?3, relay_address_count = ?4, membership_count = ?5
             WHERE singleton = 1",
            (
                observation.observed_at_ms,
                observation.ready,
                observation.direct_address_count,
                observation.relay_address_count,
                observation.membership_count,
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Replaces desired relay configuration and advances revision atomically.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when relay configuration cannot be committed.
    pub fn set_relay_configuration(
        &mut self,
        configuration: &RelayConfiguration,
    ) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "UPDATE relay_configuration SET public_fallback_enabled = ?1,
             public_relay_url = ?2, private_provider_enabled = ?3,
             listener_address = ?4, tls_mode = ?5 WHERE singleton = 1",
            (
                configuration.public_fallback_enabled,
                configuration.public_relay_url.as_deref(),
                configuration.private_provider_enabled,
                configuration.listener_address.as_deref(),
                configuration.tls_mode,
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Replaces the latest expiring observation for one relay URL.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when relay observation state cannot be committed.
    pub fn record_relay_observation(
        &mut self,
        observation: &RelayObservation,
    ) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO relay_observations(relay_url, observed_at_ms, expires_at_ms,
             reachable, latency_ms, observed_state) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(relay_url) DO UPDATE SET observed_at_ms = excluded.observed_at_ms,
             expires_at_ms = excluded.expires_at_ms, reachable = excluded.reachable,
             latency_ms = excluded.latency_ms, observed_state = excluded.observed_state",
            (
                observation.relay_url.as_str(),
                observation.observed_at_ms,
                observation.expires_at_ms,
                observation.reachable,
                observation.latency_ms,
                observation.observed_state.as_slice(),
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }
}
