use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use iroh::address_lookup::{AddressLookup, EndpointInfo, Error as LookupError, Item};
use iroh_base::{EndpointId as IrohEndpointId, TransportAddr};
use ma2a_core::{EndpointId, SpaceAuthorizationView, SpaceId};
use n0_future::{StreamExt as _, boxed::BoxStream};

use crate::{
    AddressCacheOutcome, AddressLookupExclusion, AddressMetrics, ValidatedAddressRecord,
    address_endpoint_data_to_iroh, address_observation::AddressObservation,
};

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

#[derive(Debug, Default)]
struct AuthorizationState {
    order: Vec<SpaceId>,
    by_space: BTreeMap<SpaceId, SpaceAuthorizationView>,
}

/// Private target-specific Iroh address lookup backed only by validated Space records.
#[derive(Clone, Debug)]
pub struct SpaceAddressLookup {
    records: Arc<RwLock<BTreeMap<RecordKey, ValidatedAddressRecord>>>,
    authorizations: Arc<RwLock<AuthorizationState>>,
    clock: Arc<dyn AddressLookupClock>,
    metrics: AddressMetrics,
}

#[derive(Clone, Debug)]
pub(crate) struct RuntimeAddressLookup {
    resolver: SpaceAddressLookup,
    observation: AddressObservation,
}

impl Default for SpaceAddressLookup {
    fn default() -> Self {
        Self::with_clock(Arc::new(SystemAddressLookupClock))
    }
}

impl SpaceAddressLookup {
    /// Creates an empty lookup with an injected expiry clock.
    pub fn with_clock(clock: Arc<dyn AddressLookupClock>) -> Self {
        Self::with_clock_and_metrics(clock, AddressMetrics::default())
    }

    /// Creates an empty lookup with injected clock and typed metrics.
    pub fn with_clock_and_metrics(
        clock: Arc<dyn AddressLookupClock>,
        metrics: AddressMetrics,
    ) -> Self {
        Self {
            records: Arc::new(RwLock::new(BTreeMap::new())),
            authorizations: Arc::new(RwLock::new(AuthorizationState::default())),
            clock,
            metrics,
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
        let mut replacement = AuthorizationState::default();
        for authorization in authorizations {
            let space_id = authorization.space_id();
            if replacement
                .by_space
                .insert(space_id, authorization)
                .is_some()
            {
                return Err(AddressLookupStateError);
            }
            replacement.order.push(space_id);
        }
        {
            let mut state = self
                .authorizations
                .write()
                .map_err(|_| AddressLookupStateError)?;
            *state = replacement;
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
        let mut records = self.records.write().map_err(|_| AddressLookupStateError)?;
        if let Some(current) = records.get(&key) {
            match current
                .record()
                .record()
                .sequence()
                .cmp(&record.record().record().sequence())
            {
                std::cmp::Ordering::Greater => {
                    self.metrics.record_cache(AddressCacheOutcome::Stale);
                    return Ok(());
                }
                std::cmp::Ordering::Equal => {
                    self.metrics.record_cache(AddressCacheOutcome::Equal);
                    return Ok(());
                }
                std::cmp::Ordering::Less => {}
            }
        }
        records.insert(key, record);
        self.metrics.record_cache(AddressCacheOutcome::Inserted);
        drop(records);
        Ok(())
    }

    /// Resolves only the requested Endpoint from current authorized Space-local records.
    pub fn resolve_endpoint(&self, endpoint_id: IrohEndpointId) -> Option<EndpointInfo> {
        let target = EndpointId::from(endpoint_id);
        let now_ms = self.clock.now_ms();
        let data = {
            let records = self.records.read().ok()?;
            let authorizations = self.authorizations.read().ok()?;
            let mut addresses = Vec::<TransportAddr>::new();
            let mut seen = BTreeSet::<TransportAddr>::new();
            let mut user_data = None::<Option<String>>;
            for space_id in &authorizations.order {
                let Some(validated) = records.get(&(*space_id, target)) else {
                    continue;
                };
                let Some(authorization) = authorizations.by_space.get(space_id) else {
                    continue;
                };
                let record = validated.record().record();
                if !authorization.contains_member(target) {
                    continue;
                }
                if record.issued_at_ms() > now_ms {
                    self.metrics.record_lookup(AddressLookupExclusion::Future);
                    continue;
                }
                if record.expires_at_ms() <= now_ms {
                    self.metrics.record_lookup(AddressLookupExclusion::Expired);
                    continue;
                }
                let record_user_data = record.endpoint_data().user_data().map(str::to_owned);
                if user_data
                    .as_ref()
                    .is_some_and(|current| current != &record_user_data)
                {
                    return None;
                }
                user_data = Some(record_user_data);
                for address in record.endpoint_data().addresses() {
                    if seen.insert(address.clone()) {
                        addresses.push(address.clone());
                    }
                }
            }
            drop(authorizations);
            drop(records);
            (addresses, user_data.flatten())
        };
        if data.0.is_empty() && data.1.is_none() {
            return None;
        }
        let bounded = ma2a_core::AddressEndpointDataV1::from_parts(data.0, data.1).ok()?;
        let endpoint_data = address_endpoint_data_to_iroh(&bounded).ok()?;
        Some(EndpointInfo::from_parts(endpoint_id, endpoint_data))
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

impl RuntimeAddressLookup {
    pub(crate) fn new(resolver: SpaceAddressLookup) -> Self {
        Self {
            resolver,
            observation: AddressObservation::default(),
        }
    }

    pub(crate) fn observation(&self) -> AddressObservation {
        self.observation.clone()
    }
}

impl AddressLookup for RuntimeAddressLookup {
    fn publish(&self, data: &iroh::address_lookup::EndpointData) {
        self.observation.observe(data);
    }

    fn resolve(&self, endpoint_id: IrohEndpointId) -> Option<BoxStream<Result<Item, LookupError>>> {
        self.resolver.resolve(endpoint_id)
    }
}
