//! Canonical Process v1 `open`, `accept`, `control`, and `outcome` codecs.

use iroh_base::SecretKey;
use ma2a_core::{
    EndpointId, RequestId,
    service::{
        DEFAULT_LOG_LIMIT, EndReason, LogFrom, LogWindow, MAX_LOG_LIMIT, MAX_LOGS_READ,
        MAX_TERMINATE_GRACE_MS, MIN_LOG_LIMIT, OpenBody, OpenHeader, ProcessAccept, ProcessDetails,
        ProcessId, ProcessReason, ProcessReply, ProcessRequest, ProcessSignal, ProcessSpec,
        ProcessStart, ProcessState, ProcessSummary, ServiceAccess, ServiceError, Termination,
        decode_skipped, encode_skipped,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn target() -> EndpointId {
    SecretKey::from_bytes(&[0x61; 32]).public().into()
}

fn request_id() -> TestResult<RequestId> {
    Ok(RequestId::try_from([0x62; 16].as_slice())?)
}

const fn id() -> ProcessId {
    ProcessId::from_bytes([0x63; 16])
}

fn start() -> TestResult<ProcessStart> {
    let spec = ProcessSpec::new(
        Some("worker".to_owned()),
        vec!["--serve".to_owned()],
        Some("/srv".to_owned()),
    )?
    .with_env("MODE".to_owned(), "batch".to_owned())?;
    Ok(ProcessStart::new(spec, true, DEFAULT_LOG_LIMIT)?
        .with_name("nightly build".to_owned())?
        .with_label("team".to_owned(), "infra".to_owned())?)
}

fn roundtrip(request: ProcessRequest) -> TestResult {
    let header = OpenHeader::new(request_id()?, target(), OpenBody::Process(request));
    let encoded = header.encode()?;
    assert_eq!(OpenHeader::decode(&encoded)?, header);
    let mut trailing = encoded;
    trailing.push(0);
    assert!(OpenHeader::decode(&trailing).is_err());
    Ok(())
}

#[test]
fn every_process_request_roundtrips_canonically() -> TestResult {
    for request in [
        ProcessRequest::Start(start()?),
        ProcessRequest::List(None),
        ProcessRequest::List(Some(id())),
        ProcessRequest::Inspect(id()),
        ProcessRequest::Logs(id(), LogFrom::Start, MAX_LOGS_READ),
        ProcessRequest::Logs(id(), LogFrom::Cursor(9), 1),
        ProcessRequest::Attach(id(), LogFrom::Now, true),
        ProcessRequest::Stdin(id(), false),
        ProcessRequest::Signal(id(), ProcessSignal::User2),
        ProcessRequest::Terminate(id(), MAX_TERMINATE_GRACE_MS),
        ProcessRequest::Delete(id()),
    ] {
        roundtrip(request)?;
    }
    Ok(())
}

#[test]
fn only_state_changing_process_operations_are_mutations() -> TestResult {
    assert!(ProcessRequest::Start(start()?).is_mutation());
    assert!(ProcessRequest::Attach(id(), LogFrom::Start, true).is_mutation());
    assert!(!ProcessRequest::Attach(id(), LogFrom::Start, false).is_mutation());
    assert!(!ProcessRequest::Logs(id(), LogFrom::Start, 1).is_mutation());
    assert!(!ProcessRequest::List(None).is_mutation());
    assert!(ProcessRequest::Delete(id()).is_mutation());
    assert_eq!(
        ProcessRequest::Inspect(id()).access(),
        ServiceAccess::Process
    );
    assert_eq!(ProcessRequest::Delete(id()).process_id(), Some(id()));
    Ok(())
}

#[test]
fn out_of_bound_process_requests_are_rejected() -> TestResult {
    let spec = ProcessSpec::new(Some("worker".to_owned()), Vec::new(), None)?;
    assert!(ProcessStart::new(spec.clone(), false, MIN_LOG_LIMIT - 1).is_err());
    assert!(ProcessStart::new(spec.clone(), false, MAX_LOG_LIMIT + 1).is_err());
    let shell = ProcessSpec::new(None, Vec::new(), None)?;
    assert!(ProcessStart::new(shell, false, DEFAULT_LOG_LIMIT).is_err());
    let valid = ProcessStart::new(spec, false, DEFAULT_LOG_LIMIT)?;
    assert!(valid.clone().with_name(String::new()).is_err());
    assert!(valid.clone().with_name("tab\there".to_owned()).is_err());
    assert!(
        valid
            .clone()
            .with_label("Upper".to_owned(), "x".to_owned())
            .is_err()
    );
    let labelled = valid.with_label("a".to_owned(), "1".to_owned())?;
    assert!(
        labelled
            .clone()
            .with_label("a".to_owned(), "2".to_owned())
            .is_err()
    );
    let mut full = labelled;
    for index in 1..8 {
        full = full.with_label(format!("k{index}"), String::new())?;
    }
    assert!(full.with_label("k9".to_owned(), String::new()).is_err());
    for request in [
        ProcessRequest::Logs(id(), LogFrom::Start, MAX_LOGS_READ + 1),
        ProcessRequest::Logs(id(), LogFrom::Start, 0),
        ProcessRequest::Terminate(id(), MAX_TERMINATE_GRACE_MS + 1),
    ] {
        let header = OpenHeader::new(request_id()?, target(), OpenBody::Process(request));
        assert!(OpenHeader::decode(&header.encode()?).is_err());
    }
    Ok(())
}

#[test]
fn process_ids_print_and_parse_as_lowercase_hex() -> TestResult {
    let text = id().to_string();
    assert_eq!(text, "63".repeat(16));
    assert_eq!(ProcessId::parse(&text)?, id());
    assert!(ProcessId::parse(&"63".repeat(15)).is_err());
    assert!(ProcessId::parse(&"6G".repeat(16)).is_err());
    assert!(ProcessId::parse(&"AB".repeat(16)).is_err());
    assert_ne!(ProcessId::random()?, ProcessId::random()?);
    Ok(())
}

#[test]
fn the_lifecycle_permits_only_forward_transitions() {
    use ProcessState::{Created, Exited, Failed, Lost, Running, Starting, Terminated};
    assert!(Created.permits(Starting));
    assert!(Created.permits(Failed));
    assert!(!Created.permits(Running));
    assert!(!Created.permits(Lost));
    assert!(Starting.permits(Running));
    assert!(Starting.permits(Exited));
    assert!(Running.permits(Lost));
    assert!(!Running.permits(Starting));
    for terminal in [Exited, Terminated, Failed, Lost] {
        assert!(terminal.is_terminal());
        assert!(
            ProcessState::ALL
                .iter()
                .all(|next| !terminal.permits(*next))
        );
    }
    assert_eq!(ProcessState::from_name("running"), Some(Running));
}

fn summary() -> TestResult<ProcessSummary> {
    Ok(ProcessSummary::new(id(), request_id()?, 1_700)
        .with_state(ProcessState::Exited, (None, Some(Termination::Exited(3))))
        .with_details(Some("nightly build".to_owned()), (Some(1_701), Some(1_900))))
}

#[test]
fn every_process_reply_roundtrips_canonically() -> TestResult {
    let terminated = summary()?.with_state(
        ProcessState::Terminated,
        (
            Some(ProcessReason::Requested),
            Some(Termination::Signaled(15)),
        ),
    );
    for reply in [
        Ok(ProcessReply::Summary(summary()?)),
        Ok(ProcessReply::Page(
            vec![summary()?, terminated.clone()],
            Some(id()),
        )),
        Ok(ProcessReply::Page(Vec::new(), None)),
        Ok(ProcessReply::Details(Box::new(ProcessDetails::new(
            summary()?,
            Some(start()?),
            LogWindow::new(10, 20),
        )))),
        Ok(ProcessReply::Details(Box::new(ProcessDetails::new(
            terminated.clone(),
            None,
            LogWindow::default(),
        )))),
        Ok(ProcessReply::Logs(40, true)),
        Ok(ProcessReply::Detached(EndReason::Cancelled, terminated, 41)),
        Ok(ProcessReply::Written(5, true)),
        Ok(ProcessReply::Deleted),
        Err(ServiceError::UnknownProcess),
        Err(ServiceError::WriterBusy),
    ] {
        let encoded = ProcessReply::encode_outcome(&reply)?;
        assert_eq!(ProcessReply::decode_outcome(&encoded)?, reply);
    }
    Ok(())
}

#[test]
fn a_detached_outcome_only_completes_or_cancels() -> TestResult {
    for reason in [EndReason::Completed, EndReason::Cancelled] {
        let reply = Ok(ProcessReply::Detached(reason, summary()?, 41));
        let encoded = ProcessReply::encode_outcome(&reply)?;
        assert_eq!(ProcessReply::decode_outcome(&encoded)?, reply);
    }
    // A timeout or shutdown ends an attachment with a failed outcome, never a Detached one:
    // neither is encoded, nor decoded from bytes written by hand.
    for reason in [EndReason::TimedOut, EndReason::Shutdown] {
        let reply = Ok(ProcessReply::Detached(reason, summary()?, 41));
        assert!(ProcessReply::encode_outcome(&reply).is_err(), "{reason:?}");
    }
    let completed = Ok(ProcessReply::Detached(EndReason::Completed, summary()?, 41));
    let completed = ProcessReply::encode_outcome(&completed)?;
    for reason in [&b"\x69timed_out"[..], b"\x68shutdown"] {
        let malformed = replaced(&completed, b"\x69completed", reason)?;
        assert!(ProcessReply::decode_outcome(&malformed).is_err());
    }
    Ok(())
}

/// `bytes` with its one occurrence of `from` replaced by `to`.
fn replaced(bytes: &[u8], from: &[u8], to: &[u8]) -> TestResult<Vec<u8>> {
    let at = bytes
        .windows(from.len())
        .position(|window| window == from)
        .ok_or("not found")?;
    let mut replaced = bytes.get(..at).ok_or("prefix")?.to_vec();
    replaced.extend_from_slice(to);
    replaced.extend_from_slice(bytes.get(at + from.len()..).ok_or("suffix")?);
    Ok(replaced)
}

#[test]
fn accepts_and_skip_controls_roundtrip_canonically() -> TestResult {
    for accept in [
        ProcessAccept::Started(id(), true),
        ProcessAccept::Reading {
            state: ProcessState::Running,
            start: 12,
            window: LogWindow::new(12, 30),
            truncated: true,
        },
        ProcessAccept::Admitted,
    ] {
        assert_eq!(ProcessAccept::decode(&accept.encode()?)?, accept);
    }
    let beyond = ProcessAccept::Reading {
        state: ProcessState::Running,
        start: 31,
        window: LogWindow::new(12, 30),
        truncated: false,
    };
    assert!(ProcessAccept::decode(&beyond.encode()?).is_err());
    assert_eq!(decode_skipped(&encode_skipped(77)?)?, 77);
    Ok(())
}

#[test]
fn start_bytes_are_canonical_and_stable() -> TestResult {
    let bytes = start()?.to_bytes()?;
    assert_eq!(ProcessStart::from_bytes(&bytes)?, start()?);
    let mut trailing = bytes;
    trailing.push(0);
    assert!(ProcessStart::from_bytes(&trailing).is_err());
    Ok(())
}

#[test]
fn a_label_with_an_empty_value_roundtrips() -> TestResult {
    let start = start()?.with_label("empty".to_owned(), String::new())?;
    assert_eq!(ProcessStart::from_bytes(&start.to_bytes()?)?, start);
    let header = OpenHeader::new(
        request_id()?,
        target(),
        OpenBody::Process(ProcessRequest::Start(start)),
    );
    assert_eq!(OpenHeader::decode(&header.encode()?)?, header);
    Ok(())
}
