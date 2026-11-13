//! `exec.run` and `process.*`.

use std::{
    collections::BTreeMap,
    pin::Pin,
    task::{Context, Poll},
};

use ma2a_agent::{
    protocol::{MAX_TOOL_RESULT_LEN, ToolError},
    tools::{
        ExecOutput, ExecRun, MAX_EXEC_CAPTURE, ProcessInfo, ProcessStartArgs, ProcessTarget,
        ProcessTerminate,
    },
};
use ma2a_core::service::{
    DEFAULT_LOG_LIMIT, ExecRequest, OpenBody, ProcessId, ProcessReply, ProcessRequest, ProcessSpec,
    ProcessStart, ProcessSummary, Termination,
};
use ma2a_services::client::{
    self,
    process::{ProcessControl, ProcessIo},
};
use tokio::io::AsyncWrite;

use super::{Call, Outcome, failed, ok};

/// Default `exec.run` timeout: one minute.
const DEFAULT_EXEC_TIMEOUT_MS: u64 = 60_000;
/// Default `process.terminate` grace period.
const DEFAULT_GRACE_MS: u32 = 5_000;

/// Keeps the first [`MAX_EXEC_CAPTURE`] bytes written and counts the rest.
#[derive(Debug, Default)]
struct Capture {
    bytes: Vec<u8>,
    dropped: bool,
}

impl AsyncWrite for Capture {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let room = MAX_EXEC_CAPTURE.saturating_sub(self.bytes.len());
        let kept = bytes.get(..room.min(bytes.len())).unwrap_or_default();
        self.bytes.extend_from_slice(kept);
        if kept.len() < bytes.len() {
            self.dropped = true;
        }
        Poll::Ready(Ok(bytes.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Halves `text` at a character boundary.
fn halve(text: &mut String) {
    let mut end = text.len() / 2;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

/// Cuts the longer stream in half until the whole answer fits one stored result.
fn fitted(mut output: ExecOutput) -> ExecOutput {
    let fits = |output: &ExecOutput| {
        serde_json::json!({ "ok": output }).to_string().len() <= MAX_TOOL_RESULT_LEN
    };
    while !fits(&output) && (!output.stdout.is_empty() || !output.stderr.is_empty()) {
        output.truncated = true;
        if output.stdout.len() >= output.stderr.len() {
            halve(&mut output.stdout);
        } else {
            halve(&mut output.stderr);
        }
    }
    output
}

fn invalid(what: &str) -> ToolError {
    ToolError::new("invalid_tool", format!("invalid {what}"))
}

fn spec(
    (program, args): (String, Vec<String>),
    (cwd, env): (Option<String>, BTreeMap<String, String>),
) -> Result<ProcessSpec, ToolError> {
    let mut spec = ProcessSpec::new(Some(program), args, cwd).map_err(|_| invalid("program"))?;
    for (name, value) in env {
        spec = spec
            .with_env(name, value)
            .map_err(|_| invalid("environment"))?;
    }
    Ok(spec)
}

fn process_info(summary: &ProcessSummary) -> ProcessInfo {
    ProcessInfo {
        id: summary.id.to_string(),
        state: summary.state.name().to_owned(),
        reason: summary.reason.map(|reason| reason.name().to_owned()),
        exit_code: match summary.termination {
            Some(Termination::Exited(code)) => Some(code),
            _ => None,
        },
    }
}

fn process_id(text: &str) -> Result<ProcessId, ToolError> {
    ProcessId::parse(text).map_err(|_| invalid("Process identifier"))
}

impl Call<'_> {
    pub(super) async fn exec_run(&self, args: ExecRun) -> Outcome {
        let spec = spec((args.program, args.args), (args.cwd, args.env))?;
        let timeout = args.timeout_ms.unwrap_or(DEFAULT_EXEC_TIMEOUT_MS);
        let request = ExecRequest::new(spec, false, timeout).map_err(|_| invalid("timeout"))?;
        let (mut reader, mut writer) = self.open(&args.endpoint, OpenBody::Exec(request)).await?;
        let (mut output, mut error) = (Capture::default(), Capture::default());
        let local = ProcessIo {
            input: None::<tokio::io::Empty>,
            output: &mut output,
            error: Some(&mut error),
        };
        let control = ProcessControl {
            resizes: None,
            cancel: std::future::pending::<()>(),
        };
        let streamed = client::process::run_streamed((&mut reader, &mut writer), local, control)
            .await
            .map_err(|error| failed(&error))?;
        if streamed.replayed {
            // A call re-run after a restart is answered from replay: the command already ran,
            // but its output was never retained. Empty output would read as a real result.
            return Err(ToolError::new(
                "output_unavailable",
                format!(
                    "the command already ran ({}, reason {}), but its output was not retained",
                    termination(streamed.result.termination),
                    streamed.result.reason.name()
                ),
            ));
        }
        let result = streamed.result;
        let (exit_code, signal) = match result.termination {
            Termination::Exited(code) => (Some(code), None),
            Termination::Signaled(signal) => (None, Some(signal)),
            Termination::Unknown => (None, None),
        };
        ok(&fitted(ExecOutput {
            exit_code,
            signal,
            reason: result.reason.name().to_owned(),
            stdout: String::from_utf8_lossy(&output.bytes).into_owned(),
            stderr: String::from_utf8_lossy(&error.bytes).into_owned(),
            truncated: output.dropped || error.dropped,
        }))
    }

    async fn process(&self, (endpoint, request): (&str, ProcessRequest)) -> Outcome {
        let mutation = request.is_mutation();
        let body = OpenBody::Process(request);
        let (mut reader, _writer) = if mutation {
            self.open(endpoint, body).await?
        } else {
            self.open_read(endpoint, body).await?
        };
        match client::hosted::call(&mut reader)
            .await
            .map_err(|error| failed(&error))?
        {
            (_, ProcessReply::Summary(summary)) => ok(&process_info(&summary)),
            (_, ProcessReply::Details(details)) => ok(&process_info(&details.summary)),
            _ => Err(invalid("Process answer")),
        }
    }

    pub(super) async fn process_start(&self, args: ProcessStartArgs) -> Outcome {
        let spec = spec((args.program, args.args), (args.cwd, args.env))?;
        let mut start =
            ProcessStart::new(spec, false, DEFAULT_LOG_LIMIT).map_err(|_| invalid("start"))?;
        if let Some(name) = args.name {
            start = start.with_name(name).map_err(|_| invalid("name"))?;
        }
        self.process((&args.endpoint, ProcessRequest::Start(start)))
            .await
    }

    pub(super) async fn process_inspect(&self, args: ProcessTarget) -> Outcome {
        let request = ProcessRequest::Inspect(process_id(&args.id)?);
        self.process((&args.endpoint, request)).await
    }

    pub(super) async fn process_terminate(&self, args: ProcessTerminate) -> Outcome {
        let grace = args.grace_ms.unwrap_or(DEFAULT_GRACE_MS);
        let request = ProcessRequest::Terminate(process_id(&args.id)?, grace);
        self.process((&args.endpoint, request)).await
    }
}

#[cfg(test)]
mod tests {
    use ma2a_agent::{protocol::MAX_TOOL_RESULT_LEN, tools::ExecOutput};

    use super::fitted;

    fn output(stdout: String, stderr: String) -> ExecOutput {
        ExecOutput {
            exit_code: Some(0),
            signal: None,
            reason: "exited".to_owned(),
            stdout,
            stderr,
            truncated: false,
        }
    }

    fn stored_len(output: &ExecOutput) -> usize {
        serde_json::json!({ "ok": output }).to_string().len()
    }

    #[test]
    fn exec_output_is_cut_to_fit_one_stored_result() {
        // Control characters expand sixfold when encoded.
        let cut = fitted(output("\u{1}".repeat(60_000), "é".repeat(30_000)));
        assert!(cut.truncated);
        assert!(
            stored_len(&cut) <= MAX_TOOL_RESULT_LEN,
            "{}",
            stored_len(&cut)
        );
        assert!(!cut.stdout.is_empty() && !cut.stderr.is_empty());

        let small = fitted(output("hello".to_owned(), String::new()));
        assert!(!small.truncated);
        assert_eq!(small.stdout, "hello");
    }
}

fn termination(termination: Termination) -> String {
    match termination {
        Termination::Exited(code) => format!("exit code {code}"),
        Termination::Signaled(signal) => format!("signal {signal}"),
        Termination::Unknown => "unknown termination".to_owned(),
    }
}
