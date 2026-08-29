use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const VALIDATION_OUTCOMES: usize = 11;
const PERSISTENCE_OUTCOMES: usize = 4;
const LOOKUP_EXCLUSIONS: usize = 2;
const CACHE_OUTCOMES: usize = 3;

/// Closed validation metric outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressValidationOutcome {
    /// Record passed all validation stages.
    Accepted,
    /// Canonical decoding or structural bounds failed.
    InvalidEncoding,
    /// Requested Endpoint did not match.
    WrongEndpoint,
    /// Requested Space did not match.
    WrongSpace,
    /// Endpoint was not a current member.
    UnauthorizedMember,
    /// Record was issued in the future.
    FutureRecord,
    /// Record had expired.
    ExpiredRecord,
    /// Endpoint signature was invalid.
    InvalidSignature,
    /// Persistent sequence was lower than current state.
    Rollback,
    /// Persistent sequence reused different bytes.
    Fork,
    /// Persistent state access failed.
    StoreError,
}

/// Closed persistence sequence metric outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressPersistenceOutcome {
    /// Higher sequence committed.
    Advanced,
    /// Identical current sequence replayed.
    Idempotent,
    /// Lower sequence rejected.
    Rollback,
    /// Different current-sequence bytes rejected.
    Fork,
}

/// Closed lookup-time validity exclusions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressLookupExclusion {
    /// Record issue time was after the lookup clock.
    Future,
    /// Record expiry was at or before the lookup clock.
    Expired,
}

/// Closed cache admission metric outcomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AddressCacheOutcome {
    /// Record became the cache high-water value.
    Inserted,
    /// Lower sequence was ignored.
    Stale,
    /// Equal sequence was ignored.
    Equal,
}

#[derive(Debug, Default)]
struct Counters {
    validation: [AtomicU64; VALIDATION_OUTCOMES],
    persistence: [AtomicU64; PERSISTENCE_OUTCOMES],
    lookup: [AtomicU64; LOOKUP_EXCLUSIONS],
    cache: [AtomicU64; CACHE_OUTCOMES],
}

/// Privacy-safe address metric counters with no runtime labels.
#[derive(Clone, Debug, Default)]
pub struct AddressMetrics(Arc<Counters>);

impl AddressMetrics {
    pub(crate) fn record_validation(&self, outcome: AddressValidationOutcome) {
        if let Some(counter) = self.0.validation.get(validation_index(outcome)) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_persistence(&self, outcome: AddressPersistenceOutcome) {
        if let Some(counter) = self.0.persistence.get(persistence_index(outcome)) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_lookup(&self, exclusion: AddressLookupExclusion) {
        if let Some(counter) = self.0.lookup.get(lookup_index(exclusion)) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_cache(&self, outcome: AddressCacheOutcome) {
        if let Some(counter) = self.0.cache.get(cache_index(outcome)) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Captures current low-cardinality counters.
    pub fn snapshot(&self) -> AddressMetricsSnapshot {
        AddressMetricsSnapshot {
            validation: load(&self.0.validation),
            persistence: load(&self.0.persistence),
            lookup: load(&self.0.lookup),
            cache: load(&self.0.cache),
        }
    }
}

/// Immutable typed address metric snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressMetricsSnapshot {
    validation: [u64; VALIDATION_OUTCOMES],
    persistence: [u64; PERSISTENCE_OUTCOMES],
    lookup: [u64; LOOKUP_EXCLUSIONS],
    cache: [u64; CACHE_OUTCOMES],
}

impl AddressMetricsSnapshot {
    /// Returns a validation outcome count.
    pub fn validation(&self, outcome: AddressValidationOutcome) -> u64 {
        self.validation
            .get(validation_index(outcome))
            .copied()
            .unwrap_or(0)
    }

    /// Returns a persistence outcome count.
    pub fn persistence(&self, outcome: AddressPersistenceOutcome) -> u64 {
        self.persistence
            .get(persistence_index(outcome))
            .copied()
            .unwrap_or(0)
    }

    /// Returns a lookup exclusion count.
    pub fn lookup(&self, exclusion: AddressLookupExclusion) -> u64 {
        self.lookup
            .get(lookup_index(exclusion))
            .copied()
            .unwrap_or(0)
    }

    /// Returns a cache admission count.
    pub fn cache(&self, outcome: AddressCacheOutcome) -> u64 {
        self.cache.get(cache_index(outcome)).copied().unwrap_or(0)
    }
}

fn load<const N: usize>(counters: &[AtomicU64; N]) -> [u64; N] {
    std::array::from_fn(|index| {
        counters
            .get(index)
            .map_or(0, |counter| counter.load(Ordering::Relaxed))
    })
}

const fn validation_index(outcome: AddressValidationOutcome) -> usize {
    match outcome {
        AddressValidationOutcome::Accepted => 0,
        AddressValidationOutcome::InvalidEncoding => 1,
        AddressValidationOutcome::WrongEndpoint => 2,
        AddressValidationOutcome::WrongSpace => 3,
        AddressValidationOutcome::UnauthorizedMember => 4,
        AddressValidationOutcome::FutureRecord => 5,
        AddressValidationOutcome::ExpiredRecord => 6,
        AddressValidationOutcome::InvalidSignature => 7,
        AddressValidationOutcome::Rollback => 8,
        AddressValidationOutcome::Fork => 9,
        AddressValidationOutcome::StoreError => 10,
    }
}

const fn persistence_index(outcome: AddressPersistenceOutcome) -> usize {
    match outcome {
        AddressPersistenceOutcome::Advanced => 0,
        AddressPersistenceOutcome::Idempotent => 1,
        AddressPersistenceOutcome::Rollback => 2,
        AddressPersistenceOutcome::Fork => 3,
    }
}

const fn lookup_index(exclusion: AddressLookupExclusion) -> usize {
    match exclusion {
        AddressLookupExclusion::Future => 0,
        AddressLookupExclusion::Expired => 1,
    }
}

const fn cache_index(outcome: AddressCacheOutcome) -> usize {
    match outcome {
        AddressCacheOutcome::Inserted => 0,
        AddressCacheOutcome::Stale => 1,
        AddressCacheOutcome::Equal => 2,
    }
}
