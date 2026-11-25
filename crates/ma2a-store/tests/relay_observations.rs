//! Iroh-observed relay state: bounds on new observations, and replays that change nothing.

#[path = "common/support.rs"]
mod support;

use ma2a_core::MAX_RELAY_URL_LEN;
use ma2a_store::{RelayObservation, Repository, StoreConfig};
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
