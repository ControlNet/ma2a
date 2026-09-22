//! A slow Runtime is not a broken frame.
//!
//! These tests stand a fake daemon in front of the real client transport and make
//! it answer later than the transport deadline but well inside the command
//! deadline, which is exactly what enrollment and departure do while they wait on
//! bounded remote work.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use tokio::sync::oneshot;

use crate::{
    api,
    ipc::{
        IO_DEADLINE, IpcError, IpcPaths, LocalApiClient,
        framing::{FrameRef, read_frame, write_frame},
        platform,
    },
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

static NEXT_STATE: AtomicU64 = AtomicU64::new(0);
const REQUEST_ID: &str = "3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a";
const SPACE_ID: &str = "4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b";
/// Longer than one frame transfer may take, far shorter than any command deadline.
const SLOW_RUNTIME_WORK: Duration = Duration::from_millis(2_250);

struct TempState(PathBuf);

impl TempState {
    fn new(label: &str) -> TestResult<Self> {
        let serial = NEXT_STATE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ma2a-ipc-deadline-{label}-{}-{serial}",
            std::process::id()
        ));
        fs::create_dir(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self(path))
    }
}

impl Drop for TempState {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

/// Answers the handshake at once and every other command only after `delay`.
async fn serve_slowly(listener: platform::PlatformListener, delay: Duration) -> TestResult {
    loop {
        let mut stream = platform::accept(&listener).await?;
        let frame = read_frame(&mut stream, api::MAX_LOCAL_REQUEST_BYTES).await?;
        let request: serde_json::Value = serde_json::from_slice(&frame.payload)?;
        let operation = request
            .get("operation")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let handshake = operation == "handshake";
        let payload = if handshake {
            br#"{"version":1,"request_id":null,"revision":1,"result":{"type":"handshake","payload":{}}}"#.to_vec()
        } else {
            tokio::time::sleep(delay).await;
            format!(
                r#"{{"version":1,"request_id":"{REQUEST_ID}","revision":2,"result":{{"type":"{operation}","payload":{{}}}}}}"#
            )
            .into_bytes()
        };
        write_frame(
            &mut stream,
            FrameRef {
                correlation: frame.correlation,
                payload: &payload,
                maximum: api::MAX_LOCAL_RESPONSE_BYTES,
            },
        )
        .await?;
        if !handshake {
            return Ok(());
        }
    }
}

async fn call_against_slow_runtime(label: &str, request: &str) -> TestResult<Vec<u8>> {
    let state = TempState::new(label)?;
    let paths = IpcPaths::new(&state.0)?;
    paths.prepare()?;
    let listener = platform::bind(&paths)?;
    let daemon = tokio::spawn(serve_slowly(listener, SLOW_RUNTIME_WORK));
    let command = api::decode_command(request.as_bytes())?;
    let response = LocalApiClient::new(paths).call(&command).await;
    daemon.abort();
    Ok(response?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_accept_survives_a_runtime_slower_than_the_transport_deadline() -> TestResult {
    // Given
    let request = format!(
        r#"{{"version":1,"operation":"space_redeem","request_id":"{REQUEST_ID}","invitation":"ma2ainvite-placeholder"}}"#
    );

    // When
    let response = call_against_slow_runtime("accept", &request).await?;

    // Then
    assert!(SLOW_RUNTIME_WORK > IO_DEADLINE);
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(
        response.pointer("/result/type").and_then(|v| v.as_str()),
        Some("space_redeem")
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_leave_survives_an_authority_slower_than_the_transport_deadline() -> TestResult {
    // Given
    let request = format!(
        r#"{{"version":1,"operation":"space_leave","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}"}}"#
    );

    // When
    let response = call_against_slow_runtime("leave", &request).await?;

    // Then
    assert!(SLOW_RUNTIME_WORK > IO_DEADLINE);
    let response: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(
        response.pointer("/result/type").and_then(|v| v.as_str()),
        Some("space_leave")
    );
    Ok(())
}

/// The transport deadline still governs a frame that has begun and then stalls.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_truncated_reply_is_still_reported_as_a_broken_frame() -> TestResult {
    // Given: a daemon already listening, which starts a reply and never ends it.
    let state = TempState::new("truncated")?;
    let paths = IpcPaths::new(&state.0)?;
    paths.prepare()?;
    let listener = platform::bind(&paths)?;
    let (stalled, stall_began) = oneshot::channel();
    let daemon = tokio::spawn(stall_mid_reply(listener, stalled));
    let request = format!(
        r#"{{"version":1,"operation":"space_leave","request_id":"{REQUEST_ID}","space_id":"{SPACE_ID}"}}"#
    );
    let command = api::decode_command(request.as_bytes())?;

    // When
    let started = Instant::now();
    let result = LocalApiClient::new(paths).call(&command).await;
    let elapsed = started.elapsed();
    daemon.abort();

    // Then: the partial frame really arrived, and it was the deadline that ended it.
    assert!(
        stall_began.await.is_ok(),
        "the daemon never sent the partial reply; the call failed for another reason"
    );
    assert!(
        matches!(result, Err(IpcError::InvalidFrame)),
        "expected a broken frame, got {result:?}"
    );
    assert!(
        elapsed >= IO_DEADLINE,
        "the call ended after {elapsed:?}, before the frame deadline could have"
    );
    Ok(())
}

/// Answers the handshake, then announces a reply frame and never finishes it.
async fn stall_mid_reply(
    listener: platform::PlatformListener,
    stalled: oneshot::Sender<()>,
) -> TestResult {
    loop {
        let mut stream = platform::accept(&listener).await?;
        let frame = read_frame(&mut stream, api::MAX_LOCAL_REQUEST_BYTES).await?;
        let request: serde_json::Value = serde_json::from_slice(&frame.payload)?;
        if request.get("operation").and_then(serde_json::Value::as_str) == Some("handshake") {
            let payload = br#"{"version":1,"request_id":null,"revision":1,"result":{"type":"handshake","payload":{}}}"#;
            write_frame(
                &mut stream,
                FrameRef {
                    correlation: frame.correlation,
                    payload,
                    maximum: api::MAX_LOCAL_RESPONSE_BYTES,
                },
            )
            .await?;
            continue;
        }
        tokio::io::AsyncWriteExt::write_all(&mut stream, &64_u32.to_be_bytes()).await?;
        tokio::io::AsyncWriteExt::flush(&mut stream).await?;
        let _observed = stalled.send(());
        tokio::time::sleep(IO_DEADLINE * 4).await;
        return Ok(());
    }
}
