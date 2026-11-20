use std::sync::Arc;

use serde_json::{Map, Value, json};

use ma2a_runtime::ipc::IpcError;

use super::{Notice, ServiceHost, covered, notice};
use crate::{Call, Ended, Error};

async fn reply(call: Call) -> Result<Ended, Error> {
    call.reply(b"").await
}

fn manifest(versions: &Value) -> Map<String, Value> {
    let mut manifest = Map::new();
    manifest.insert("versions".to_owned(), versions.clone());
    manifest
}

#[test]
fn handlers_take_valid_operation_names_and_versions_from_one() {
    assert!(ServiceHost::new().handle((1, "echo"), reply).is_ok());
    assert!(matches!(
        ServiceHost::new().handle((0, "echo"), reply),
        Err(Error::Protocol(_))
    ));
    assert!(matches!(
        ServiceHost::new().handle((1, "Not An Operation"), reply),
        Err(Error::Protocol(_))
    ));
}

#[test]
fn a_manifest_operation_without_a_handler_is_refused_before_serving() -> Result<(), Error> {
    let host = ServiceHost::new()
        .handle((1, "echo"), reply)?
        .handle((2, "echo"), reply)?;
    let handlers = Arc::new(host.handlers);
    let served = json!([
        {"version": 1, "operations": [{"name": "echo", "interaction": "unary"}]},
        {"version": 2, "operations": [{"name": "echo", "interaction": "unary"}]},
    ]);
    covered(&manifest(&served), &handlers)?;
    let missing = json!([
        {"version": 2, "operations": [
            {"name": "echo", "interaction": "unary"},
            {"name": "count", "interaction": "stream"},
        ]},
    ]);
    match covered(&manifest(&missing), &handlers) {
        Err(Error::MissingHandler { version, operation }) => {
            assert_eq!((version, operation.as_str()), (2, "count"));
        }
        other => panic!("expected a missing handler, got {other:?}"),
    }
    assert!(matches!(
        covered(&Map::new(), &handlers),
        Err(Error::Protocol(_))
    ));
    Ok(())
}

#[test]
fn link_messages_are_exactly_a_call_or_a_stop() {
    let id = "0123456789abcdef0123456789abcdef";
    let call = format!(r#"{{"t":"call","call":"{id}"}}"#);
    assert!(matches!(notice(call.as_bytes()), Some(Notice::Call(bytes)) if bytes[0] == 0x01));
    assert!(matches!(notice(br#"{"t":"stop"}"#), Some(Notice::Stop)));
    for malformed in [
        &br#"{"t":"call","call":"0123"}"#[..],
        br#"{"t":"call"}"#,
        br#"{"t":"ready"}"#,
        b"not json",
    ] {
        assert!(notice(malformed).is_none());
    }
}

#[test]
fn only_an_attach_that_may_succeed_later_is_retried() {
    let unavailable = || Error::Refused {
        error: "unavailable".to_owned(),
        remediation: None,
    };
    let unregistered = Error::Refused {
        error: "not_found".to_owned(),
        remediation: None,
    };
    let lost = std::io::Error::from(std::io::ErrorKind::ConnectionRefused);
    for transient in [
        Error::Ipc(IpcError::Io(lost)),
        Error::Ipc(IpcError::InvalidFrame),
        unavailable(),
    ] {
        assert!(transient.retries(), "{transient} should be retried");
    }
    for lasting in [
        Error::Ipc(IpcError::VersionMismatch),
        Error::Ipc(IpcError::UnauthorizedPeer),
        Error::Ipc(IpcError::InvalidPath),
        unregistered,
        Error::Protocol("the Runtime attached without a link"),
        Error::MissingHandler {
            version: 1,
            operation: "echo".to_owned(),
        },
    ] {
        assert!(!lasting.retries(), "{lasting} should be returned");
    }
}
