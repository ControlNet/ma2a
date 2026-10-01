//! Supervisor records and requests: canonical encodings and refusal of anything else.

#[path = "support/host.rs"]
mod support;

use std::path::PathBuf;

use ma2a_core::service::{
    LogFrom, ProcessId, ProcessSignal, ProcessSpec, ProcessStart, ServiceError, Termination,
};
use ma2a_services::supervisor::{
    Credentials, EndCause, Launch, SupervisorAddress, SupervisorRequest, SupervisorStatus,
    protocol::{decode_reading, decode_written, encode_reading, encode_written},
    status,
};
use support::{TempRoot, TestResult};

fn statuses() -> Vec<SupervisorStatus> {
    vec![
        SupervisorStatus::Awaiting,
        SupervisorStatus::Running(4242, 1_700_000_000_000),
        SupervisorStatus::Ended(Termination::Exited(3), EndCause::Exited, 9),
        SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 10),
        SupervisorStatus::Ended(Termination::Unknown, EndCause::Exited, 11),
        SupervisorStatus::SpawnFailed(12),
        SupervisorStatus::NeverStarted(13),
    ]
}

#[test]
fn statuses_roundtrip_and_only_ends_are_final() -> TestResult {
    for status in statuses() {
        assert_eq!(SupervisorStatus::from_bytes(&status.to_bytes())?, status);
        let live = matches!(
            status,
            SupervisorStatus::Awaiting | SupervisorStatus::Running(..)
        );
        assert_eq!(status.is_final(), !live);
        let mut longer = status.to_bytes();
        longer.push(0);
        assert!(SupervisorStatus::from_bytes(&longer).is_err());
        let bytes = status.to_bytes();
        let shorter = bytes.get(..bytes.len() - 1).ok_or("empty")?;
        assert!(SupervisorStatus::from_bytes(shorter).is_err());
    }
    assert!(SupervisorStatus::from_bytes(b"MA2AST1\x09").is_err());
    assert!(SupervisorStatus::from_bytes(b"garbage").is_err());
    Ok(())
}

#[test]
fn the_status_record_is_replaced_atomically() -> TestResult {
    let root = TempRoot::new("status-record")?;
    assert_eq!(status::read(root.path())?, None);
    for next in statuses() {
        status::write(root.path(), next)?;
        assert_eq!(status::read(root.path())?, Some(next));
    }
    std::fs::write(root.path().join("status"), b"not a status")?;
    assert!(status::read(root.path()).is_err());
    Ok(())
}

fn launch(directory: PathBuf) -> TestResult<Launch> {
    let spec = ProcessSpec::new(Some("/bin/echo".to_owned()), vec!["hi".to_owned()], None)?;
    let start = ProcessStart::new(spec, true, 2_097_152)?
        .with_name("build".to_owned())?
        .with_label("team".to_owned(), "core".to_owned())?;
    Ok(Launch {
        credentials: Credentials::new(ProcessId::random()?, [9; 32]),
        directory,
        address: SupervisorAddress::new("/run/ma2a/p/x.sock".to_owned()),
        start,
    })
}

#[test]
fn launch_records_roundtrip_and_keep_the_secret_out_of_debug() -> TestResult {
    let record = launch(std::env::temp_dir().join("ma2a-launch"))?;
    let bytes = record.to_bytes()?;
    assert_eq!(Launch::from_bytes(&bytes)?, record);
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(Launch::from_bytes(&trailing).is_err());
    let truncated = bytes.get(..bytes.len() - 1).ok_or("empty")?;
    assert!(Launch::from_bytes(truncated).is_err());
    assert!(Launch::from_bytes(b"MA2APL0").is_err());

    let relative = launch(PathBuf::from("relative/dir"))?.to_bytes()?;
    assert!(Launch::from_bytes(&relative).is_err());

    let shown = format!("{:?}", record.credentials);
    assert!(shown.contains("redacted"));
    assert!(!shown.contains("9, 9"));
    Ok(())
}

#[test]
fn requests_roundtrip_and_unknown_bytes_are_refused() -> TestResult {
    let mut requests = vec![
        SupervisorRequest::Watch,
        SupervisorRequest::Go,
        SupervisorRequest::Abort,
        SupervisorRequest::Stdin(false),
        SupervisorRequest::Stdin(true),
        SupervisorRequest::Terminate(0),
        SupervisorRequest::Terminate(60_000),
    ];
    for from in [LogFrom::Start, LogFrom::Now, LogFrom::Cursor(77)] {
        requests.push(SupervisorRequest::Attach(from, false));
        requests.push(SupervisorRequest::Attach(from, true));
    }
    requests.extend(ProcessSignal::ALL.map(SupervisorRequest::Signal));
    for request in requests {
        assert_eq!(SupervisorRequest::from_bytes(&request.to_bytes())?, request);
        let mut longer = request.to_bytes();
        longer.push(0);
        assert_eq!(
            SupervisorRequest::from_bytes(&longer),
            Err(ServiceError::InvalidInput)
        );
    }
    for bad in [&[][..], &[7], &[3, 2], &[4, 7], &[2, 3, 0]] {
        assert!(SupervisorRequest::from_bytes(bad).is_err(), "{bad:?}");
    }
    Ok(())
}

#[test]
fn attach_and_stdin_replies_roundtrip() -> TestResult {
    let reading = encode_reading((5, 3, 9), true);
    assert_eq!(decode_reading(&reading)?, ((5, 3, 9), true));
    assert!(decode_reading(reading.get(..reading.len() - 1).ok_or("empty")?).is_err());
    let written = encode_written(12, false);
    assert_eq!(decode_written(&written)?, (12, false));
    assert_eq!(decode_written(&encode_written(0, true))?, (0, true));
    assert!(decode_written(&[0; 10]).is_err());
    Ok(())
}
