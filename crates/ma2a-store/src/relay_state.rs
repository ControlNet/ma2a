use ma2a_core::MAX_PRIVATE_RELAY_URL_LEN;

use crate::repository::increment_revision;
use crate::{RelayObservation, Repository, StoreError};

impl Repository {
    /// Atomically replaces current Iroh-observed relay state without changing desired config.
    ///
    /// # Errors
    /// Returns [`StoreError`] when observation persistence fails.
    pub fn replace_relay_observations(
        &mut self,
        observations: &[RelayObservation],
    ) -> Result<u64, StoreError> {
        let transaction = self.immediate()?;
        transaction.execute("DELETE FROM relay_observations", [])?;
        for observation in observations {
            transaction.execute(
                "INSERT INTO relay_observations(relay_url, observed_at_ms, expires_at_ms,
                 reachable, latency_ms, observed_state) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (
                    observation.relay_url.as_str(),
                    observation.observed_at_ms,
                    observation.expires_at_ms,
                    observation.reachable,
                    observation.latency_ms,
                    observation.observed_state.as_slice(),
                ),
            )?;
        }
        let revision = increment_revision(&transaction)?;
        transaction.commit()?;
        Ok(revision)
    }

    /// Loads current Iroh-observed relay state separately from desired candidates.
    ///
    /// # Errors
    /// Returns [`StoreError`] when persisted observations are malformed or unreadable.
    pub fn relay_observations(&self) -> Result<Vec<RelayObservation>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT relay_url, observed_at_ms, expires_at_ms, reachable, latency_ms,
             observed_state FROM relay_observations ORDER BY relay_url",
        )?;
        statement
            .query_map([], |row| {
                Ok(RelayObservation {
                    relay_url: row.get(0)?,
                    observed_at_ms: row.get(1)?,
                    expires_at_ms: row.get(2)?,
                    reachable: row.get(3)?,
                    latency_ms: row.get(4)?,
                    observed_state: row.get(5)?,
                })
            })?
            .map(|row| {
                let observation = row?;
                if observation.relay_url.len() > MAX_PRIVATE_RELAY_URL_LEN {
                    return Err(StoreError::SchemaMismatch {
                        detail: "persisted relay observation URL exceeds protocol bounds",
                    });
                }
                Ok(observation)
            })
            .collect()
    }
}
