//! Remote services through this Runtime's local stream plane.
//!
//! The CLI never holds Endpoint identity: each operation upgrades one local API connection,
//! and the daemon dials the target as itself and relays the Services v1 frames.

use std::io;

use ma2a_core::{
    RequestId,
    service::{FrameKind, OpenBody, OpenHeader, RemotePath, ServiceError},
};
use ma2a_runtime::ipc::{IpcPaths, LocalApiClient, LocalStream, StreamUpgrade};
use ma2a_services::{FrameReader, FrameWriter, client::ClientError};
use serde_json::Value;
use tokio::io::{ReadHalf, WriteHalf};

use crate::{
    AppError,
    commands::workflows::{command, request_fields},
};

pub(crate) mod agent;
mod agent_view;
#[cfg(all(test, unix))]
mod deadline_tests;
pub(crate) mod files;
#[cfg(all(test, unix))]
mod follow_tests;
pub(crate) mod hosted;
mod hosted_stream;
mod hosted_view;
pub(crate) mod process;
pub(crate) mod proxy;
#[cfg(all(test, unix))]
mod reply_tests;
pub(crate) mod service;
mod transfer;

/// The caller's frame halves of one opened operation.
pub(crate) type Halves = (
    FrameReader<ReadHalf<LocalStream>>,
    FrameWriter<WriteHalf<LocalStream>>,
);

impl From<ClientError> for AppError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Local(error) => Self::Io(error),
            other => Self::Protocol(other.to_string()),
        }
    }
}

/// Longest wait for a read-only remote operation, dial included, so a target that never
/// answers fails the command rather than holding it forever.
const READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// Runs `read` under `deadline`, failing as `timed_out` once it passes.
async fn bounded<T>(
    deadline: std::time::Duration,
    read: impl Future<Output = Result<T, AppError>>,
) -> Result<T, AppError> {
    tokio::time::timeout(deadline, read)
        .await
        .unwrap_or_else(|_| Err(ClientError::Stream(ServiceError::TimedOut).into()))
}

/// Opens one remote operation on `endpoint` and sends its `open`.
pub(crate) async fn open(
    paths: &IpcPaths,
    endpoint: &str,
    body: OpenBody,
) -> Result<Halves, AppError> {
    open_as(paths, (endpoint, None), body).await
}

/// Opens one remote operation under `request_id`, or a fresh one when it is `None`.
pub(crate) async fn open_as(
    paths: &IpcPaths,
    target: (&str, Option<&str>),
    body: OpenBody,
) -> Result<Halves, AppError> {
    Ok(open_bound(paths, target, body).await?.1)
}

/// Opens one remote operation as [`open_as`] does, and returns the `RequestId` it was opened
/// under, which the answer to a creating operation names.
pub(crate) async fn open_bound(
    paths: &IpcPaths,
    (endpoint, request_id): (&str, Option<&str>),
    body: OpenBody,
) -> Result<(RequestId, Halves), AppError> {
    crate::daemon_control::require(paths).await?;
    let mut fields = request_fields()?;
    if let Some(request_id) = request_id {
        fields.insert(
            "request_id".to_owned(),
            Value::String(request_id.to_owned()),
        );
    }
    fields.insert(
        "target_endpoint_id".to_owned(),
        Value::String(endpoint.to_owned()),
    );
    fields.insert(
        "service".to_owned(),
        Value::String(body.kind().name().to_owned()),
    );
    let upgrade = command("stream_open", fields)?;
    let (request_id, target, _) = upgrade
        .stream_open()
        .ok_or_else(|| io::Error::other("stream_open did not decode as an upgrade"))?;
    let stream = match LocalApiClient::new(paths.clone())
        .open_stream(&upgrade)
        .await?
    {
        StreamUpgrade::Opened(stream) => stream,
        StreamUpgrade::Refused(response) => {
            let document: Value = serde_json::from_slice(&response).map_err(io::Error::other)?;
            crate::output::reject_runtime_error(&document)?;
            return Err(io::Error::other("daemon refused the stream without an error").into());
        }
        _ => return Err(io::Error::other("unsupported stream upgrade answer").into()),
    };
    let (read, write) = tokio::io::split(stream);
    let (reader, mut writer) = (FrameReader::new(read), FrameWriter::new(write));
    let header = OpenHeader::new(request_id, target, body)
        .encode()
        .map_err(|_| AppError::Usage("the remote request exceeds a protocol bound"))?;
    let sent = async {
        writer.frame(FrameKind::Open, &header).await?;
        writer.flush().await
    };
    sent.await.map_err(ClientError::Stream)?;
    Ok((request_id, (reader, writer)))
}

/// Ends the process with `outcome`, reported as `main` reports it.
///
/// A command that read stdin ends this way: a pending read of stdin cannot be cancelled and
/// would hold the runtime open after `main` returned. So does the daemon, once it closed its
/// Runtime: a blocking job stuck on a hung file system would hold it open just the same.
pub(crate) fn exit_with(outcome: Result<(), AppError>) -> ! {
    let code = match outcome {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            i32::from(error.exit_code())
        }
    };
    let _flushed = io::Write::flush(&mut io::stdout().lock());
    let _flushed = io::Write::flush(&mut io::stderr().lock());
    std::process::exit(code)
}

/// Passes `result` on, except that an error while stdin is being read ends the process.
pub(crate) fn unless_reading_stdin<T>(
    reading: bool,
    result: Result<T, AppError>,
) -> Result<T, AppError> {
    match result {
        Err(error) if reading => exit_with(Err(error)),
        other => other,
    }
}

/// Parses a `ROOT:/dir/file` remote path.
pub(crate) fn remote_path(text: &str) -> Result<RemotePath, AppError> {
    RemotePath::parse(text).map_err(|_| {
        AppError::Invalid(
            "invalid remote path; use ROOT:/dir/file with a valid root name".to_owned(),
        )
    })
}

/// Parses a duration such as `500ms`, `30s`, `10m`, or `2h` within `1ms..=maximum_ms`.
pub(crate) fn duration_ms(value: &str, maximum_ms: u64) -> Result<u64, AppError> {
    duration_within(value, 1..=maximum_ms)
}

/// Parses a duration as [`duration_ms`] does, within `bounds`.
pub(crate) fn duration_within(
    value: &str,
    bounds: std::ops::RangeInclusive<u64>,
) -> Result<u64, AppError> {
    let units = [("ms", 1), ("s", 1_000), ("m", 60_000), ("h", 3_600_000)];
    units
        .iter()
        .find_map(|(suffix, multiplier)| {
            value
                .strip_suffix(suffix)
                .and_then(|amount| amount.parse::<u64>().ok())
                .and_then(|amount| amount.checked_mul(*multiplier))
        })
        .filter(|duration| bounds.contains(duration))
        .ok_or(AppError::Usage(
            "DURATION must be a number ending in ms, s, m, or h, within the service's bound",
        ))
}

#[cfg(test)]
mod tests {
    use super::duration_ms;

    #[test]
    fn durations_take_one_unit_suffix_and_stay_inside_their_bound() {
        assert_eq!(duration_ms("500ms", 10_000).ok(), Some(500));
        assert_eq!(duration_ms("30s", 60_000).ok(), Some(30_000));
        assert_eq!(duration_ms("10m", 3_600_000).ok(), Some(600_000));
        assert_eq!(duration_ms("2h", 86_400_000).ok(), Some(7_200_000));
        assert!(duration_ms("2h", 3_600_000).is_err());
        assert!(duration_ms("0s", 1_000).is_err());
        assert!(duration_ms("10", 1_000).is_err());
        assert!(duration_ms("m", 1_000).is_err());
    }
}
