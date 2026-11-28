use std::path::Path;

use super::{parse_duration_ms, retry_hint, validate_request_id};

#[test]
fn the_default_invite_lifetime_is_five_minutes() {
    // Given / When / Then
    assert_eq!(
        parse_duration_ms(crate::cli::DEFAULT_INVITE_TTL).ok(),
        Some(300_000)
    );
}

#[test]
fn invite_lifetimes_stay_inside_the_protocol_bound() {
    // Given / When / Then
    assert_eq!(parse_duration_ms("30s").ok(), Some(30_000));
    assert_eq!(parse_duration_ms("1ms").ok(), Some(1));
    assert!(parse_duration_ms("6m").is_err());
    assert!(parse_duration_ms("0s").is_err());
    assert!(parse_duration_ms("5").is_err());
}

#[test]
fn explicit_accept_request_id_requires_32_lowercase_hex_chars() {
    let valid = "0123456789abcdef0123456789abcdef";
    assert_eq!(
        validate_request_id(valid.to_owned()).ok().as_deref(),
        Some(valid)
    );
    assert!(validate_request_id("0123".to_owned()).is_err());
    assert!(validate_request_id("0123456789ABCDEF0123456789ABCDEF".to_owned()).is_err());
}

#[test]
fn retry_hint_quotes_state_dir_and_falls_back_on_control_chars() {
    let id = "0123456789abcdef0123456789abcdef";
    #[cfg(not(windows))]
    assert_eq!(
        retry_hint(Path::new("/tmp/my state/o'ne"), id),
        format!(
            "Retry the exact same ticket with: ma2a --state-dir '/tmp/my state/o'\\''ne' space accept --request-id {id}"
        )
    );
    #[cfg(windows)]
    assert_eq!(
        retry_hint(Path::new(r"C:\ma2a\my state"), id),
        format!(
            "Retry the exact same ticket with: ma2a --state-dir \"C:\\ma2a\\my state\" space accept --request-id {id}"
        )
    );
    assert_eq!(
        retry_hint(Path::new("/tmp/bad\nstate"), id),
        format!(
            "Keep the original --state-dir and retry the exact same ticket with: ma2a space accept --request-id {id}"
        )
    );
}
