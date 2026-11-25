//! Tests for keeping only the usable parts of a stored relay configuration.

use ma2a_store::{RelayConfiguration, RelayTransportConfiguration};

use super::usable_relay_configuration;

fn public(url: &str) -> RelayConfiguration {
    RelayConfiguration {
        public_fallback_enabled: true,
        public_relay_urls: vec![url.to_owned()],
        private_provider_enabled: false,
        listener_address: None,
        private_relay_url: None,
        served_spaces: Vec::new(),
        transport: None,
    }
}

fn with_private(configuration: RelayConfiguration, url: &str) -> RelayConfiguration {
    RelayConfiguration {
        private_provider_enabled: true,
        listener_address: Some("127.0.0.1:3340".to_owned()),
        private_relay_url: Some(url.to_owned()),
        served_spaces: vec![ma2a_core::SpaceId::derive(b"relay-recovery")],
        transport: Some(RelayTransportConfiguration::ExternalTlsTermination),
        ..configuration
    }
}

#[test]
fn each_part_is_kept_only_while_the_runtime_can_use_it() {
    let (good, credentials) = (
        "https://relay.example.invalid/",
        "https://user:secret@relay.example.invalid/",
    );
    let both = with_private(public(good), good);
    assert_eq!(usable_relay_configuration(&both), both);

    let bad_private = with_private(public(good), credentials);
    assert_eq!(usable_relay_configuration(&bad_private), public(good));

    let bad_public = with_private(public(credentials), good);
    let kept = usable_relay_configuration(&bad_public);
    assert!(!kept.public_fallback_enabled && kept.public_relay_urls.is_empty());
    assert_eq!(kept.private_relay_url.as_deref(), Some(good));

    let neither = with_private(public(credentials), credentials);
    assert_eq!(
        usable_relay_configuration(&neither),
        RelayConfiguration {
            public_fallback_enabled: false,
            public_relay_urls: Vec::new(),
            ..public(good)
        }
    );
}
