use ma2a_core::{MAX_RELAY_URL_LEN, ProtocolError};

use crate::repository::increment_revision;
use crate::{RelayObservation, Repository, StoreError};

impl Repository {
    /// Atomically replaces current Iroh-observed relay state without changing desired config.
    ///
    /// # Errors
    /// Returns [`StoreError`] when an observation's relay URL is over the bound or observation
    /// persistence fails.
    pub fn replace_relay_observations(
        &mut self,
        observations: &[RelayObservation],
    ) -> Result<u64, StoreError> {
        observations
            .iter()
            .try_for_each(|observation| bounded(&observation.relay_url))?;
        // A replay after a lost reply must not advance revision, whatever order it lists the
        // relays in. Observation time is bookkeeping; relay identity and reachability are the
        // state. Stored rows are compared as they are, so one written before the URL bound
        // existed reads as a change and is replaced instead of failing the replacement.
        let mut requested = observations.iter().collect::<Vec<_>>();
        requested.sort_unstable_by(|left, right| left.relay_url.cmp(&right.relay_url));
        // The comparison runs inside the write transaction, so two writers making the same
        // change at once commit it once.
        let transaction = self.immediate()?;
        let current = stored_relay_observations(&transaction)?;
        if current.len() == requested.len()
            && current.iter().zip(requested).all(|(old, new)| {
                old.relay_url == new.relay_url
                    && old.reachable == new.reachable
                    && old.latency_ms == new.latency_ms
                    && old.observed_state == new.observed_state
            })
        {
            return Ok(transaction.query_row(
                "SELECT revision FROM runtime_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )?);
        }
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
        let observations = stored_relay_observations(&self.connection)?;
        if observations
            .iter()
            .any(|observation| observation.relay_url.len() > MAX_RELAY_URL_LEN)
        {
            return Err(StoreError::SchemaMismatch {
                detail: "persisted relay observation URL exceeds protocol bounds",
            });
        }
        Ok(observations)
    }
}

fn stored_relay_observations(
    connection: &rusqlite::Connection,
) -> Result<Vec<RelayObservation>, StoreError> {
    let mut statement = connection.prepare(
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
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

/// Refuses a relay URL longer than an address record can carry before anything is written.
pub(crate) fn bounded(relay_url: &str) -> Result<(), StoreError> {
    if relay_url.len() > MAX_RELAY_URL_LEN {
        return Err(ProtocolError::INVALID_INPUT.into());
    }
    Ok(())
}
