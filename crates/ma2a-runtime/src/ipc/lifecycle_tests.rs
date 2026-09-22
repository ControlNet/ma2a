use ma2a_core::EndpointId;

use super::{DaemonIdentity, DaemonReport, LifecycleRequest, ProcessIncarnation, RuntimeBoot};

fn identity(nonce: Option<&str>) -> DaemonIdentity {
    let endpoint = EndpointId::from(iroh::SecretKey::generate().public());
    DaemonIdentity::new(
        nonce.map(str::to_owned),
        ProcessIncarnation::recorded(4242, Some(99)),
        RuntimeBoot::new([7_u8; 16], endpoint),
    )
}

#[test]
fn a_ping_round_trips_through_the_fixed_envelope() {
    // Given
    let encoded = LifecycleRequest::Ping.encode();

    // Then
    assert_eq!(
        LifecycleRequest::parse(&encoded),
        Some(LifecycleRequest::Ping)
    );
}

#[test]
fn a_stop_round_trips_through_the_fixed_envelope() {
    // Given
    let encoded = LifecycleRequest::Stop.encode();

    // Then
    assert_eq!(
        LifecycleRequest::parse(&encoded),
        Some(LifecycleRequest::Stop)
    );
}

#[test]
fn ordinary_local_api_traffic_is_not_mistaken_for_a_lifecycle_call() {
    // Given
    let handshake = br#"{"version":1,"operation":"handshake"}"#;
    let error = br#"{"version":1,"error":"invalid_input","remediation":null}"#;

    // Then
    assert_eq!(LifecycleRequest::parse(handshake), None);
    assert_eq!(LifecycleRequest::parse(error), None);
    assert_eq!(LifecycleRequest::parse(b"not json"), None);
}

#[test]
fn a_readiness_line_carries_the_launch_it_belongs_to() {
    // Given
    let identity = identity(Some("0123456789abcdef0123456789abcdef"));

    // When
    let line = identity.ready_line();
    let report = DaemonReport::parse(line.as_bytes()).expect("readiness record");

    // Then
    assert!(line.ends_with('\n'));
    assert_eq!(report.operation(), "ready");
    assert_eq!(
        report.launch_nonce(),
        Some("0123456789abcdef0123456789abcdef")
    );
    assert_eq!(report.incarnation().pid(), 4242);
    assert_eq!(report.incarnation().started_at(), Some(99));
    assert_eq!(report.runtime_boot_id(), "07070707070707070707070707070707");
}

#[test]
fn a_foreground_daemon_reports_readiness_without_a_launch() {
    // Given
    let report =
        DaemonReport::parse(identity(None).ready_line().as_bytes()).expect("readiness record");

    // Then
    assert_eq!(report.launch_nonce(), None);
}

#[test]
fn a_reply_and_the_metadata_file_describe_the_same_daemon() {
    // Given
    let identity = identity(Some("00000000000000000000000000000001"));

    // When
    let pong = DaemonReport::parse(&identity.reply("pong")).expect("pong");
    let recorded = DaemonReport::parse(identity.metadata().as_bytes()).expect("metadata");

    // Then
    assert_eq!(pong.operation(), "pong");
    assert_eq!(recorded.operation(), "running");
    assert_eq!(pong.endpoint_id(), recorded.endpoint_id());
    assert_eq!(pong.launch_nonce(), recorded.launch_nonce());
    assert_eq!(pong.api_version(), u64::from(crate::api::LOCAL_API_VERSION));
}

#[test]
fn a_record_with_a_malformed_identifier_is_refused() {
    // Given
    let truncated = br#"{"ma2a_lifecycle":1,"op":"pong","api_version":1,"binary_version":"0",
        "pid":1,"process_started_at":null,"launch_nonce":null,
        "runtime_boot_id":"07","endpoint_id":"07"}"#;

    // Then
    assert_eq!(DaemonReport::parse(truncated), None);
}
