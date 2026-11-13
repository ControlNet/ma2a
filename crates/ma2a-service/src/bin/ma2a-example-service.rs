//! `ma2a-example-service`: a deterministic example hosted service and an example of the SDK.
//!
//! It is an example, not a production service. Version 2 adds operations to version 1, so a
//! caller that selects version 1 keeps working unchanged:
//!
//! | Version | Operation | Interaction | Effect |
//! | --- | --- | --- | --- |
//! | 1, 2 | `echo` | unary | replies with the body |
//! | 1, 2 | `upper` | unary | replies with the body in ASCII upper case |
//! | 2 | `count` | stream | body `N` (1–1000): sends `1\n` … `N\n` on output 50 ms apart, then replies `N`; a cancel ends it `cancelled` |
//! | 2 | `hold` | resource | accepts and holds until the caller cancels, then ends `cancelled` |
//! | 2 | `pipe` | bytes | copies input (channel 0) to output (channel 1) until input ends, then replies with the byte count |
//!
//! ```text
//! ma2a-example-service manifest [MAX_CALLS]    print the manifest to register (default 4 calls)
//! ma2a-example-service serve STATE_DIR [NAME]  serve NAME (default `example`) through that Runtime
//! ```

use std::{process::ExitCode, time::Duration};

use ma2a_core::service::MAX_SERVICE_CALLS;
use ma2a_runtime::ipc::IpcPaths;
use ma2a_service::{
    Call, CallInput, CallOutput, Channel, Ended, Error, Input, ServiceError, ServiceHost,
    ServiceName,
};
use serde_json::json;

/// Largest `count` this example streams.
const MAX_COUNT: u32 = 1_000;
/// Pause between two `count` lines.
const TICK: Duration = Duration::from_millis(50);

fn manifest(max_calls: u8) -> serde_json::Value {
    let unary = |name: &str| json!({"name": name, "interaction": "unary"});
    let first = vec![unary("echo"), unary("upper")];
    let mut second = first.clone();
    second.push(json!({"name": "count", "interaction": "stream"}));
    second.push(json!({"name": "hold", "interaction": "resource"}));
    second.push(json!({"name": "pipe", "interaction": "bytes"}));
    json!({
        "description": "Deterministic example service: echo, upper, count, hold, and pipe.",
        "max_calls": max_calls,
        "versions": [
            {"version": 1, "operations": first},
            {"version": 2, "operations": second},
        ],
    })
}

/// Parses `MAX_CALLS`, which a manifest bounds to `1..=MAX_SERVICE_CALLS`.
fn max_calls(text: &str) -> Result<u8, String> {
    text.parse::<u8>()
        .ok()
        .filter(|calls| (1..=MAX_SERVICE_CALLS).contains(calls))
        .ok_or_else(|| format!("MAX_CALLS must be 1-{MAX_SERVICE_CALLS}"))
}

fn print_manifest(max_calls: u8) {
    println!("{}", manifest(max_calls));
}

async fn echo(call: Call) -> Result<Ended, Error> {
    let body = call.body().to_vec();
    call.reply(&body).await
}

async fn upper(call: Call) -> Result<Ended, Error> {
    let body = call.body().to_ascii_uppercase();
    call.reply(&body).await
}

async fn count(call: Call) -> Result<Ended, Error> {
    let limit = std::str::from_utf8(call.body())
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
        .filter(|limit| (1..=MAX_COUNT).contains(limit));
    let Some(limit) = limit else {
        return call.reject(ServiceError::InvalidInput).await;
    };
    let (mut input, mut output) = call.split();
    output.accept(b"").await?;
    for line in 1..=limit {
        tokio::select! {
            biased;
            received = input.recv() => {
                if cancelled(&received) {
                    return output.fail(ServiceError::Cancelled).await;
                }
            }
            () = tokio::time::sleep(TICK) => {
                output.send(Channel::Output, format!("{line}\n").as_bytes()).await?;
            }
        }
    }
    output.end(Channel::Output).await?;
    output.reply(limit.to_string().as_bytes()).await
}

/// Whether one caller frame ends the call; other input is ignored by `count` and `hold`.
const fn cancelled(received: &Result<Input, Error>) -> bool {
    !matches!(
        received,
        Ok(Input::Data(..) | Input::End(_) | Input::Control(_))
    )
}

async fn hold(call: Call) -> Result<Ended, Error> {
    let (mut input, mut output) = call.split();
    output.accept(b"held").await?;
    while !cancelled(&input.recv().await) {}
    output.fail(ServiceError::Cancelled).await
}

async fn pipe(call: Call) -> Result<Ended, Error> {
    let (mut input, mut output): (CallInput, CallOutput) = call.split();
    output.accept(b"").await?;
    let mut total: u64 = 0;
    loop {
        match input.recv().await? {
            Input::Data(Channel::Input, bytes) => {
                total += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                output.send(Channel::Output, &bytes).await?;
            }
            Input::End(Channel::Input) => break,
            Input::Cancelled => return output.fail(ServiceError::Cancelled).await,
            _ => {}
        }
    }
    output.end(Channel::Output).await?;
    output.reply(total.to_string().as_bytes()).await
}

fn host() -> Result<ServiceHost, Error> {
    let mut host = ServiceHost::new();
    for version in [1, 2] {
        host = host
            .handle((version, "echo"), echo)?
            .handle((version, "upper"), upper)?;
    }
    host.handle((2, "count"), count)?
        .handle((2, "hold"), hold)?
        .handle((2, "pipe"), pipe)
}

async fn serve(state_dir: &str, name: &str) -> Result<(), String> {
    let paths = IpcPaths::new(state_dir).map_err(|error| error.to_string())?;
    let name = ServiceName::new(name).map_err(|_| format!("invalid service name {name}"))?;
    let host = host().map_err(|error| error.to_string())?;
    host.serve(paths, &name)
        .await
        .map_err(|error| error.to_string())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let outcome = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["manifest"] => {
            print_manifest(4);
            Ok(())
        }
        ["manifest", calls] => max_calls(calls).map(print_manifest),
        ["serve", state_dir] => serve(state_dir, "example").await,
        ["serve", state_dir, name] => serve(state_dir, name).await,
        _ => Err(
            "usage: ma2a-example-service manifest [MAX_CALLS] | serve STATE_DIR [NAME]".to_owned(),
        ),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ma2a-example-service: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::max_calls;

    #[test]
    fn max_calls_accepts_only_what_a_manifest_can_hold() {
        assert_eq!(max_calls("1"), Ok(1));
        assert_eq!(max_calls("64"), Ok(64));
        for refused in ["0", "65", "100", "-1", "four"] {
            assert!(max_calls(refused).is_err(), "{refused}");
        }
    }
}
