use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use iroh::address_lookup::{AddressLookup, EndpointData, EndpointInfo, Error as LookupError, Item};
use iroh_base::{EndpointId as IrohEndpointId, TransportAddr};
use ma2a_core::{EndpointId, SpaceAuthorizationView, SpaceId};
use n0_future::{StreamExt as _, boxed::BoxStream};

use crate::ValidatedAddressRecord;

/// Clock used to expire cached address records during synchronous Iroh lookup.
pub trait AddressLookupClock: fmt::Debug + Send + Sync + 'static {
    /// Returns Unix time in milliseconds.
    fn now_ms(&self) -> u64;
}

#[derive(Debug)]
struct SystemAddressLookupClock;

impl AddressLookupClock for SystemAddressLookupClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            })
    }
}

/// Poisoned private lookup state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AddressLookupStateError;

impl fmt::Display for AddressLookupStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("private address lookup state is unavailable")
    }
}

impl Error for AddressLookupStateError {}

type RecordKey = (SpaceId, EndpointId);

/// Private target-specific Iroh address lookup backed only by validated Space records.
#[derive(Clone, Debug)]
pub struct SpaceAddressLookup {
    records: Arc<RwLock<BTreeMap<RecordKey, ValidatedAddressRecord>>>,
    authorizations: Arc<RwLock<BTreeMap<SpaceId, SpaceAuthorizationView>>>,
    clock: Arc<dyn AddressLookupClock>,
}

impl Default for SpaceAddressLookup {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemAddressLookupClock))
    }
}

impl SpaceAddressLookup {
    /// Creates an empty lookup with an injected expiry clock.
    pub fn with_clock(clock: Arc<dyn AddressLookupClock>) -> Self {
        Self {
            records: Arc::new(RwLock::new(BTreeMap::new())),
            authorizations: Arc::new(RwLock::new(BTreeMap::new())),
            clock,
        }
    }

    /// Replaces current verified Space authorization snapshots.
    ///
    /// # Errors
    /// Returns [`AddressLookupStateError`] when shared lookup state is poisoned.
    pub fn replace_authorizations(
        &self,
        authorizations: Vec<SpaceAuthorizationView>,
    ) -> Result<(), AddressLookupStateError> {
        {
            let mut state = self
                .authorizations
                .write()
                .map_err(|_| AddressLookupStateError)?;
            *state = authorizations
                .into_iter()
                .map(|authorization| (authorization.space_id(), authorization))
                .collect();
        }
        Ok(())
    }

    /// Caches one record that already passed ordered validation and persistence.
    ///
    /// # Errors
    /// Returns [`AddressLookupStateError`] when shared lookup state is poisoned.
    pub fn cache(&self, record: ValidatedAddressRecord) -> Result<(), AddressLookupStateError> {
        let key = (
            record.record().record().space_id(),
            record.record().record().endpoint_id(),
        );
        self.records
            .write()
            .map_err(|_| AddressLookupStateError)?
            .insert(key, record);
        Ok(())
    }

    /// Resolves only the requested Endpoint from current authorized Space-local records.
    pub fn resolve_endpoint(&self, endpoint_id: IrohEndpointId) -> Option<EndpointInfo> {
        let target = EndpointId::from(endpoint_id);
        let now_ms = self.clock.now_ms();
        let addresses = {
            let records = self.records.read().ok()?;
            let authorizations = self.authorizations.read().ok()?;
            let mut addresses = BTreeSet::<TransportAddr>::new();
            for ((space_id, record_endpoint), validated) in records.iter() {
                if *record_endpoint != target {
                    continue;
                }
                let Some(authorization) = authorizations.get(space_id) else {
                    continue;
                };
                let record = validated.record().record();
                if !authorization.contains_member(target) || record.expires_at_ms() <= now_ms {
                    continue;
                }
                addresses.extend(record.endpoint_data().addresses().iter().cloned());
            }
            drop(authorizations);
            drop(records);
            addresses
        };
        if addresses.is_empty() {
            return None;
        }
        Some(EndpointInfo::from_parts(
            endpoint_id,
            EndpointData::from(addresses),
        ))
    }
}

impl AddressLookup for SpaceAddressLookup {
    fn resolve(&self, endpoint_id: IrohEndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
        let stream = self.resolve_endpoint(endpoint_id).map_or_else(
            || n0_future::stream::empty().boxed(),
            |info| n0_future::stream::iter([Ok(Item::new(info, "ma2a_space", None))]).boxed(),
        );
        Some(stream)
    }
}
