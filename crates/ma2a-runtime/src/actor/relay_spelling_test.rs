//! A relay URL is saved in the parsed spelling the Runtime uses, not as the operator typed it.
use ma2a_store::{RelayConfiguration, Repository, StoreConfig};

use super::relay_lifecycle_test::TempState;
use crate::Runtime;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configured_relay_url_is_saved_and_reported_as_parsed() -> TestResult {
    // Given: a bare origin with an upper-case host, which parses to a lower-case URL ending
    // in a slash.
    let state = TempState::new("relay-spelling")?;
    let config = StoreConfig::new(&state.0);
    let runtime = Runtime::start(config.clone()).await?;
    let current = Repository::open(&config)?.relay_configuration()?;

    // When
    let committed = runtime
        .handle()
        .set_relay_configuration_committed(RelayConfiguration {
            public_fallback_enabled: true,
            public_relay_urls: vec!["https://RELAY.example.invalid".to_owned()],
            ..current
        })
        .await?;
    runtime.shutdown().await?;

    // Then: status and the Store both hold the parsed URL, so a restart has nothing to rewrite.
    let parsed = vec!["https://relay.example.invalid/".to_owned()];
    assert_eq!(committed.value().configuration.public_relay_urls, parsed);
    let repository = Repository::open(&config)?;
    assert_eq!(repository.relay_configuration()?.public_relay_urls, parsed);
    Ok(())
}
