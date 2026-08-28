use crate::{
    MemberRecord, MemberRevocation, RelayConfiguration, RelayObservation, Repository,
    RuntimeMetadataUpdate, StoreError, repository::increment_revision,
};

impl Repository {
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

    /// Inserts or updates current membership at an accepted manifest generation.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when membership state cannot be committed.
    pub fn upsert_member(&mut self, member: &MemberRecord) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO members(space_id, endpoint_id, role, accepted_generation)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT(space_id, endpoint_id) DO UPDATE SET
             role = excluded.role, accepted_generation = excluded.accepted_generation",
            (
                member.space_id.as_bytes().as_slice(),
                member.endpoint_id.as_bytes().as_slice(),
                member.role.code(),
                member.accepted_generation,
            ),
        )?;
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Stores the latest signed revocation for one Space member.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when revocation state cannot be committed.
    pub fn revoke_member(&mut self, revocation: &MemberRevocation) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute(
            "INSERT INTO member_revocations(space_id, endpoint_id, revoked_generation,
             signed_revocation) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(space_id, endpoint_id) DO UPDATE SET
             revoked_generation = excluded.revoked_generation,
             signed_revocation = excluded.signed_revocation",
            (
                revocation.space_id.as_bytes().as_slice(),
                revocation.endpoint_id.as_bytes().as_slice(),
                revocation.revoked_generation,
                revocation.signed_revocation.as_slice(),
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
