//! A small SDK for Agents written in Rust: the stdio loop, delivery deduplication, and
//! one [`Task`] handle per delivered task with tool calls as futures.
//!
//! ```no_run
//! # async fn example() -> std::io::Result<()> {
//! use ma2a_agent::{protocol::Message, sdk};
//!
//! sdk::serve(|task: sdk::Task| async move {
//!     let reply = format!("you said: {}", task.input().text());
//!     task.complete(Some(Message::agent_text(reply)));
//! })
//! .await
//! # }
//! ```

mod task;

use std::{
    collections::HashMap,
    future::Future,
    io,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use serde_json::Value;
use tokio::{
    io::{
        AsyncBufRead, AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader,
    },
    sync::{mpsc, oneshot, watch},
};

pub use task::Task;

use crate::protocol::{
    AgentToRuntime, MAX_AGENT_LINE_LEN, MAX_RUNTIME_LINE_LEN, Message, PROTOCOL_VERSION,
    RuntimeToAgent, ToolError, decode_line, encode_line,
};

/// What one open tool call resolves to.
type CallAnswer = oneshot::Sender<Result<Value, ToolError>>;

/// The Agent side of one task the instance holds.
#[derive(Debug)]
struct Slot {
    inbox: mpsc::UnboundedSender<Message>,
    cancel: watch::Sender<bool>,
    calls: HashMap<String, CallAnswer>,
}

/// State shared by the loop and every task handle.
#[derive(Debug)]
struct Shared {
    out: mpsc::UnboundedSender<Vec<u8>>,
    tasks: Mutex<HashMap<String, Slot>>,
}

impl Shared {
    fn tasks(&self) -> MutexGuard<'_, HashMap<String, Slot>> {
        self.tasks.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Writes one line; a line over the protocol bound is never sent.
    fn emit(&self, message: &AgentToRuntime) -> bool {
        let Ok(line) = encode_line(message) else {
            return false;
        };
        if line.len() > MAX_AGENT_LINE_LEN + 1 {
            return false;
        }
        self.out.send(line).is_ok()
    }
}

/// Runs an Agent on this process's stdin and stdout until stdin ends.
///
/// `handler` runs once per delivered task, concurrently with every other task.
///
/// # Errors
/// Returns a stdin or stdout failure.
pub async fn serve<H, F>(handler: H) -> io::Result<()>
where
    H: Fn(Task) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    serve_on((tokio::io::stdin(), tokio::io::stdout()), handler).await
}

/// Runs an Agent on any line transport, as [`serve`] does on stdio.
///
/// # Errors
/// Returns a read or write failure of the transport.
pub async fn serve_on<R, W, H, F>((input, output): (R, W), handler: H) -> io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
    H: Fn(Task) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    let (out, lines) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        out,
        tasks: Mutex::new(HashMap::new()),
    });
    let writer = tokio::spawn(write_lines(output, lines));
    shared.emit(&AgentToRuntime::Ready {
        protocol: PROTOCOL_VERSION,
    });
    let mut input = BufReader::new(input);
    let mut processed = 0_u64;
    let ended = loop {
        let line = match read_line(&mut input).await {
            Ok(Some(line)) => line,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };
        // Empty and malformed lines are skipped: the Runtime starts every connection with
        // an empty line, which ends any line a lost connection cut short.
        let Ok(delivery) = decode_line::<RuntimeToAgent>(&line) else {
            continue;
        };
        let seq = delivery.seq();
        if seq > processed {
            dispatch(&shared, delivery, &handler);
            processed = seq;
        }
        shared.emit(&AgentToRuntime::Ack { seq: processed });
    };
    // Without input no task can make progress, so nothing waits for the tasks still held.
    drop(shared);
    writer.abort();
    ended
}

/// Hands one new delivery to its task.
fn dispatch<H, F>(shared: &Arc<Shared>, delivery: RuntimeToAgent, handler: &H)
where
    H: Fn(Task) -> F,
    F: Future<Output = ()> + Send + 'static,
{
    match delivery {
        RuntimeToAgent::Task {
            task,
            from,
            message,
            ..
        } => {
            let (inbox, messages) = mpsc::unbounded_channel();
            let (cancel, cancelled) = watch::channel(false);
            let slot = Slot {
                inbox,
                cancel,
                calls: HashMap::new(),
            };
            if shared.tasks().insert(task.clone(), slot).is_some() {
                return;
            }
            let handle = Task::new(
                (task, from, message),
                Arc::clone(shared),
                (messages, cancelled),
            );
            tokio::spawn(handler(handle));
        }
        RuntimeToAgent::Message { task, message, .. } => {
            if let Some(slot) = shared.tasks().get(&task) {
                let _delivered = slot.inbox.send(message);
            }
        }
        RuntimeToAgent::Cancel { task, .. } => {
            if let Some(slot) = shared.tasks().get(&task) {
                slot.cancel.send_replace(true);
            }
        }
        RuntimeToAgent::ToolResult {
            task,
            call,
            ok,
            error,
            ..
        } => {
            let answer = shared
                .tasks()
                .get_mut(&task)
                .and_then(|slot| slot.calls.remove(&call));
            let outcome = match (ok, error) {
                (_, Some(error)) => Err(error),
                (Some(value), None) => Ok(value),
                (None, None) => Err(ToolError::new("invalid_tool", "empty tool result")),
            };
            if let Some(answer) = answer {
                let _answered = answer.send(outcome);
            }
        }
    }
}

/// Reads one line without its newline, or `None` at end of input; a line over the bound
/// is consumed and returned empty, so it is skipped.
async fn read_line<R: AsyncBufRead + Unpin>(input: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let mut overflow = false;
    loop {
        let buffer = input.fill_buf().await?;
        if buffer.is_empty() {
            // A last line without its newline still counts as one line.
            return Ok((!line.is_empty() || overflow).then_some(line));
        }
        let (chunk, done) = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or((buffer.len(), false), |end| (end, true));
        if !overflow {
            line.extend_from_slice(buffer.get(..chunk).unwrap_or_default());
            if line.len() > MAX_RUNTIME_LINE_LEN {
                overflow = true;
                line = Vec::new();
            }
        }
        input.consume(if done { chunk + 1 } else { chunk });
        if done {
            return Ok(Some(line));
        }
    }
}

async fn write_lines<W: AsyncWrite + Unpin>(
    mut output: W,
    mut lines: mpsc::UnboundedReceiver<Vec<u8>>,
) -> io::Result<()> {
    while let Some(line) = lines.recv().await {
        output.write_all(&line).await?;
        output.flush().await?;
    }
    Ok(())
}
