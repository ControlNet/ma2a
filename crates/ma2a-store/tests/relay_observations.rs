//! Relay and Endpoint state: bounds on new URLs, and observation replays that change nothing.

#[path = "common/support.rs"]
mod support;

use ma2a_core::MAX_RELAY_URL_LEN;
use ma2a_store::{
    EndpointObservationUpdate, RelayConfiguration, RelayObservation, Repository, StoreConfig,
};
use rusqlite::Connection;
use support::{TempState, TestResult};

fn observation(relay_url: String, latency_ms: u64) -> RelayObservation {
    RelayObservation {
        relay_url,
        observed_at_ms: 70,
        expires_at_ms: 90,
        reachable: true,
        latency_ms: Some(latency_ms),
        observed_state: b"observed".to_vec(),
    }
}

fn overlong_url() -> String {
    format!("https://relay.invalid/{}", "a".repeat(MAX_RELAY_URL_LEN))
}

#[test]
fn an_overlong_observation_already_stored_is_cleared_by_a_replacement() -> TestResult {
    // Given an observation longer than the bound, written before the bound existed.
    let state = TempState::new("relay-observation-overlong")?;
    let config = StoreConfig::new(state.path());
    drop(Repository::open(&config)?);
    let connection = Connection::open(config.database_path())?;
    connection.execute(
        "INSERT INTO relay_observations(relay_url, observed_at_ms, expires_at_ms, reachable,
         latency_ms, observed_state) VALUES (?1, 1, 2, 1, NULL, x'')",
        [overlong_url()],
    )?;
    drop(connection);
    let mut repository = Repository::open(&config)?;

    // When the Runtime clears observations at startup, as it does on every start.
    repository.replace_relay_observations(&[])?;

    // Then the old observation is gone and later reads succeed.
    assert!(repository.relay_observations()?.is_empty());
    Ok(())
}

#[test]
fn a_new_overlong_observation_is_refused_without_changing_anything() -> TestResult {
    // Given
    let state = TempState::new("relay-observation-refused")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let kept = observation("https://relay.invalid/".to_owned(), 4);
    repository.replace_relay_observations(std::slice::from_ref(&kept))?;
    let revision = repository.revision()?;

    // When
    let replaced = repository.replace_relay_observations(&[observation(overlong_url(), 4)]);
    let recorded = repository.record_relay_observation(&observation(overlong_url(), 4));

    // Then
    assert!(replaced.is_err());
    assert!(recorded.is_err());
    assert_eq!(repository.revision()?, revision);
    assert_eq!(repository.relay_observations()?, vec![kept]);
    Ok(())
}

#[test]
fn replaying_the_same_observations_in_any_order_keeps_the_revision() -> TestResult {
    // Given two observations in descending URL order.
    let state = TempState::new("relay-observation-replay")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let first = observation("https://relay-b.invalid/".to_owned(), 4);
    let second = observation("https://relay-a.invalid/".to_owned(), 5);
    let revision = repository.replace_relay_observations(&[first.clone(), second.clone()])?;

    // When the same batch is replayed, in the same and in the other order.
    let same = repository.replace_relay_observations(&[first.clone(), second.clone()])?;
    let swapped = repository.replace_relay_observations(&[second.clone(), first.clone()])?;
    let changed =
        repository.replace_relay_observations(&[first, observation(second.relay_url, 6)])?;

    // Then only the changed batch advances the revision, and only once.
    assert_eq!(same, revision);
    assert_eq!(swapped, revision);
    assert_eq!(changed, revision + 1);
    Ok(())
}

#[test]
fn a_relay_configuration_with_an_overlong_url_is_refused() -> TestResult {
    // Given
    let state = TempState::new("relay-configuration-refused")?;
    let config = StoreConfig::new(state.path());
    let mut repository = Repository::open(&config)?;
    let revision = repository.revision()?;
    let public = RelayConfiguration {
        public_fallback_enabled: true,
        public_relay_urls: vec![overlong_url()],
        ..repository.relay_configuration()?
    };
    let private = RelayConfiguration {
        private_relay_url: Some(overlong_url()),
        ..repository.relay_configuration()?
    };

    // When
    let public = repository.set_relay_configuration(&public);
    let private = repository.set_relay_configuration(&private);

    // Then
    assert!(public.is_err());
    assert!(private.is_err());
    assert_eq!(repository.revision()?, revision);
    assert!(!repository.relay_configuration()?.public_fallback_enabled);
    Ok(())
}

#[test]
fn concurrent_identical_replacements_advance_the_revision_once() -> TestResult {
    // Given two connections to one Store, and a third holding the write lock.
    let state = TempState::new("relay-observation-concurrent")?;
    let config = StoreConfig::new(state.path());
    let repositories = [Repository::open(&config)?, Repository::open(&config)?];
    let before = repositories[0].revision()?;
    let holder = Connection::open(config.database_path())?;
    holder.execute_batch("BEGIN IMMEDIATE")?;

    // When both replace the empty set with the same row while the lock is held.
    let writers = repositories.map(|mut repository| {
        std::thread::spawn(move || {
            repository
                .replace_relay_observations(&[observation("https://relay.invalid/".to_owned(), 4)])
        })
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    holder.execute_batch("COMMIT")?;
    let mut revisions = Vec::new();
    for writer in writers {
        revisions.push(writer.join().map_err(|_| "writer panicked")??);
    }

    // Then the change is committed once, and both callers see that one revision.
    assert_eq!(revisions, vec![before + 1, before + 1]);
    Ok(())
}

#[test]
fn concurrent_identical_endpoint_observations_advance_the_revision_once() -> TestResult {
    // Given two connections to one Store, and a third holding the write lock.
    let state = TempState::new("endpoint-observation-concurrent")?;
    let config = StoreConfig::new(state.path());
    let repositories = [Repository::open(&config)?, Repository::open(&config)?];
    let before = repositories[0].revision()?;
    let holder = Connection::open(config.database_path())?;
    holder.execute_batch("BEGIN IMMEDIATE")?;

    // When both record the same changed observation while the lock is held.
    let writers = repositories.map(|mut repository| {
        std::thread::spawn(move || {
            repository.record_endpoint_observation_if_changed(&EndpointObservationUpdate {
                observed_at_ms: 70,
                ready: true,
                direct_address_count: 2,
                relay_address_count: 1,
                membership_count: 3,
            })
        })
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    holder.execute_batch("COMMIT")?;
    let mut revisions = Vec::new();
    for writer in writers {
        revisions.push(writer.join().map_err(|_| "writer panicked")??);
    }

    // Then the change is committed once, and both callers see that one revision.
    assert_eq!(revisions, vec![before + 1, before + 1]);
    Ok(())
}
