use std::collections::BTreeSet;

use ma2a_core::{EndpointId, SpaceId};

use crate::repository::increment_revision;
use crate::{Repository, StoreError};

impl Repository {
    /// Reconciles active visibility for one provider without deleting accepted high-water rows.
    ///
    /// # Errors
    /// Returns [`StoreError`] when activity state cannot be read or committed.
    pub fn reconcile_private_relay_activity(
        &mut self,
        provider_endpoint_id: EndpointId,
        active_spaces: &[SpaceId],
    ) -> Result<(u64, bool), StoreError> {
        let active_spaces = active_spaces.iter().copied().collect::<BTreeSet<_>>();
        let transaction = self.immediate()?;
        let rows = {
            let mut statement = transaction.prepare(
                "SELECT space_id, active FROM relay_advertisement_state
                 WHERE relay_endpoint_id = ?1",
            )?;
            statement
                .query_map([provider_endpoint_id.as_bytes().as_slice()], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut changed = false;
        for (space_id, active) in rows {
            let space_id =
                SpaceId::try_from(space_id.as_slice()).map_err(|_| StoreError::SchemaMismatch {
                    detail: "persisted relay advertisement Space identifier is invalid",
                })?;
            let desired = active_spaces.contains(&space_id);
            if active != desired {
                transaction.execute(
                    "UPDATE relay_advertisement_state SET active = ?1
                     WHERE space_id = ?2 AND relay_endpoint_id = ?3",
                    (
                        desired,
                        space_id.as_bytes().as_slice(),
                        provider_endpoint_id.as_bytes().as_slice(),
                    ),
                )?;
                changed = true;
            }
        }
        let revision = if changed {
            increment_revision(&transaction)?
        } else {
            transaction.query_row(
                "SELECT revision FROM runtime_metadata WHERE singleton = 1",
                [],
                |row| row.get(0),
            )?
        };
        transaction.commit()?;
        Ok((revision, changed))
    }
}
