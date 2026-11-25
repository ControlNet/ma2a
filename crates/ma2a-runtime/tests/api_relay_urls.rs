//! Relay URLs are refused at decode time when the Runtime could not use them as configured.

use ma2a_core::{MAX_RELAY_URL_LEN, ProtocolError};
use ma2a_runtime::api::{PublicRelayView, decode_command};

const REQUEST_ID: &str = "000102030405060708090a0b0c0d0e0f";
const SPACE_ID: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

fn private_configure(url: &str) -> String {
    format!(
        r#"{{"version":1,"operation":"private_relay_configure","request_id":"{REQUEST_ID}","mode":"external_termination","listen":"127.0.0.1:3340","public_url":"{url}","served_space_ids":["{SPACE_ID}"],"certificate_path":null,"private_key_path":null}}"#
    )
}

fn public_configure(url: &str) -> String {
    format!(
        r#"{{"version":1,"operation":"public_relay_configure","request_id":"{REQUEST_ID}","url":"{url}"}}"#
    )
}

/// An HTTPS origin exactly `length` bytes long, written without the `/` parsing appends.
fn bare_origin(length: usize) -> String {
    let mut host = String::new();
    while host.len() < length - "https://".len() {
        if !host.is_empty() {
            host.push('.');
        }
        let label = (length - "https://".len() - host.len()).min(40);
        host.push_str(&"a".repeat(label));
    }
    let origin = format!("https://{host}");
    assert_eq!(origin.len(), length);
    origin
}

#[test]
fn relay_urls_the_runtime_cannot_use_are_refused_before_execution() {
    let refused = [
        "https://user:secret@relay.example.invalid/".to_owned(),
        "https://user@relay.example.invalid/".to_owned(),
        "http://relay.example.invalid/".to_owned(),
        "not a url".to_owned(),
        // Parsing appends `/`, which takes this one past the bound.
        bare_origin(MAX_RELAY_URL_LEN),
    ];
    for url in &refused {
        for input in [private_configure(url), public_configure(url)] {
            let error = decode_command(input.as_bytes()).expect_err(&input);
            assert_eq!(error.code(), ProtocolError::INVALID_INPUT, "{input}");
        }
    }
}

#[test]
fn relay_urls_the_runtime_can_use_are_accepted() {
    let accepted = [
        "https://relay.example.invalid/path@1".to_owned(),
        bare_origin(MAX_RELAY_URL_LEN - 1),
    ];
    for url in &accepted {
        for input in [private_configure(url), public_configure(url)] {
            assert!(decode_command(input.as_bytes()).is_ok(), "{input}");
        }
        assert!(PublicRelayView::new(true, Some(url.clone()), false).is_ok());
    }
}
