//! Splitting an instance's output into adapter lines and decoding each into its effect.

use ma2a_agent::protocol::{
    AgentToRuntime, MAX_AGENT_LINE_LEN, MAX_TOOL_ARGS_LEN, MAX_TOOL_NAME_LEN, PROTOCOL_VERSION,
    ReportedState, decode_line, is_call_id,
};
use ma2a_core::service::{AgentTaskId, AgentTaskState, MAX_TASK_NOTE_LEN};
use ma2a_store::AgentOutputEffect;

use super::{
    convert::from_wire,
    tools::{ended, rejection},
};

/// Cuts output into lines and knows the output position just past each one.
#[derive(Debug)]
pub(super) struct Lines {
    buffer: Vec<u8>,
    overflow: bool,
    cursor: u64,
}

/// One complete line, or `None` for a line over the bound, and the cursor past it.
pub(super) type Line = (Option<Vec<u8>>, u64);

impl Lines {
    /// Starts at `cursor`, which must be just past a newline (or the very start).
    pub(super) const fn new(cursor: u64) -> Self {
        Self {
            buffer: Vec::new(),
            overflow: false,
            cursor,
        }
    }

    /// Takes stdout bytes that begin at the current cursor.
    pub(super) fn output(&mut self, mut bytes: &[u8]) -> Vec<Line> {
        let mut lines = Vec::new();
        while !bytes.is_empty() {
            let (chunk, newline) = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or((bytes.len(), false), |end| (end, true));
            let (head, tail) = bytes.split_at(chunk);
            if !self.overflow {
                self.buffer.extend_from_slice(head);
                if self.buffer.len() > MAX_AGENT_LINE_LEN {
                    self.overflow = true;
                    self.buffer = Vec::new();
                }
            }
            self.cursor += u64::try_from(chunk).unwrap_or(u64::MAX);
            bytes = tail;
            if newline {
                self.cursor += 1;
                bytes = bytes.get(1..).unwrap_or_default();
                let line = (!self.overflow).then(|| std::mem::take(&mut self.buffer));
                self.overflow = false;
                self.buffer.clear();
                lines.push((line, self.cursor));
            }
        }
        lines
    }

    /// Takes bytes of another channel, which only move the cursor.
    pub(super) fn other(&mut self, length: usize) {
        self.cursor += u64::try_from(length).unwrap_or(u64::MAX);
    }

    /// Output before `cursor` is no longer retained: a cut line is dropped.
    pub(super) fn skipped(&mut self, cursor: u64) {
        self.cursor = cursor;
        self.buffer.clear();
        self.overflow = true;
    }

    /// Returns the position the next byte has.
    #[cfg(test)]
    pub(super) const fn cursor(&self) -> u64 {
        self.cursor
    }
}

/// Cuts one note to the durable bound at a character boundary.
fn note(mut text: String) -> String {
    if text.len() > MAX_TASK_NOTE_LEN {
        let mut end = MAX_TASK_NOTE_LEN;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Decodes one line; anything malformed, unknown, or unusable is [`AgentOutputEffect::Ignore`].
pub(super) fn effect(line: Option<&[u8]>) -> AgentOutputEffect {
    let Some(message) = line.and_then(|line| decode_line::<AgentToRuntime>(line).ok()) else {
        return AgentOutputEffect::Ignore;
    };
    decoded(message).unwrap_or(AgentOutputEffect::Ignore)
}

fn decoded(message: AgentToRuntime) -> Option<AgentOutputEffect> {
    let task = |text: &str| AgentTaskId::parse(text).ok();
    Some(match message {
        AgentToRuntime::Ready { protocol } => {
            (protocol == PROTOCOL_VERSION).then_some(AgentOutputEffect::Ready)?
        }
        AgentToRuntime::Ack { seq } => AgentOutputEffect::Ack(seq),
        AgentToRuntime::State {
            task: id,
            state,
            message,
            error,
        } => AgentOutputEffect::State {
            task: task(&id)?,
            // An invalid message invalidates the whole report.
            message: match message {
                Some(message) => Some(from_wire(&message).ok()?),
                None => None,
            },
            state: match state {
                ReportedState::Running => AgentTaskState::Running,
                ReportedState::Waiting => AgentTaskState::Waiting,
                ReportedState::Completed => AgentTaskState::Completed,
                ReportedState::Failed => AgentTaskState::Failed,
                ReportedState::Cancelled => AgentTaskState::Cancelled,
            },
            error: error.map(note),
        },
        AgentToRuntime::Message { task: id, message } => {
            AgentOutputEffect::Message(task(&id)?, from_wire(&message).ok()?)
        }
        AgentToRuntime::Progress { task: id, text } => {
            AgentOutputEffect::Progress(task(&id)?, note(text))
        }
        AgentToRuntime::Tool {
            task: id,
            call,
            name,
            args,
        } => AgentOutputEffect::ToolCall {
            task: task(&id)?,
            rejection: rejection(),
            ended: ended(),
            request: serde_json::to_vec(&args)
                .ok()
                .filter(|request| request.len() <= MAX_TOOL_ARGS_LEN)?,
            call: is_call_id(&call).then_some(call)?,
            name: (1..=MAX_TOOL_NAME_LEN)
                .contains(&name.len())
                .then_some(name)?,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::{Lines, effect};
    use ma2a_store::AgentOutputEffect;

    #[test]
    fn the_sdk_and_the_runtime_bound_messages_alike() {
        use ma2a_core::service::{MAX_MESSAGE_BYTES, MAX_MESSAGE_PARTS};
        assert_eq!(ma2a_agent::protocol::MAX_MESSAGE_BYTES, MAX_MESSAGE_BYTES);
        assert_eq!(ma2a_agent::protocol::MAX_MESSAGE_PARTS, MAX_MESSAGE_PARTS);
    }

    #[tokio::test]
    async fn an_sdk_result_beyond_the_message_bound_still_ends_its_task()
    -> Result<(), Box<dyn std::error::Error>> {
        use ma2a_agent::{
            protocol::{AgentToRuntime, Message, Origin, RuntimeToAgent, decode_line, encode_line},
            sdk,
        };
        use ma2a_core::service::{AgentTaskId, AgentTaskState};
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

        let (mut input, sdk_input) = tokio::io::duplex(1 << 16);
        let (sdk_output, output) = tokio::io::duplex(1 << 16);
        let serving = tokio::spawn(sdk::serve_on((sdk_input, sdk_output), |task| async move {
            let text = "x".repeat(ma2a_core::service::MAX_MESSAGE_BYTES + 1);
            task.complete(Some(Message::agent_text(text)));
        }));
        let id = AgentTaskId::from_bytes([2; 16]);
        let task = RuntimeToAgent::Task {
            seq: 1,
            task: id.to_string(),
            from: Origin {
                endpoint: "creator".to_owned(),
                agent: None,
            },
            message: Message::user_text("produce an inline result"),
        };
        input.write_all(&encode_line(&task)?).await?;
        let mut output = BufReader::new(output);
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let mut line = String::new();
                output.read_line(&mut line).await?;
                let line = line.trim_end().as_bytes();
                if let Ok(AgentToRuntime::State { .. }) = decode_line::<AgentToRuntime>(line) {
                    return Ok::<_, std::io::Error>(effect(Some(line)));
                }
            }
        })
        .await??;
        serving.abort();
        assert!(
            matches!(terminal, AgentOutputEffect::State { task, state: AgentTaskState::Failed, .. } if task == id),
            "{terminal:?}"
        );
        Ok(())
    }

    #[test]
    fn lines_know_the_cursor_past_each_newline_across_chunks_and_channels() {
        let mut lines = Lines::new(10);
        assert!(lines.output(b"{\"t\":").is_empty());
        lines.other(4);
        let complete = lines.output(b"\"ack\",\"seq\":1}\n{\"t\"");
        assert_eq!(
            complete,
            [(Some(b"{\"t\":\"ack\",\"seq\":1}".to_vec()), 10 + 5 + 4 + 15)]
        );
        assert_eq!(lines.cursor(), 34 + 4);
        // A skip drops the cut line; the next complete one decodes again.
        lines.skipped(100);
        let after = lines.output(b"tail\n{}\n");
        assert_eq!(after, [(None, 105), (Some(b"{}".to_vec()), 108)]);
    }

    #[test]
    fn overlong_lines_and_malformed_messages_are_ignored() {
        let mut lines = Lines::new(0);
        let long = vec![b'x'; ma2a_agent::protocol::MAX_AGENT_LINE_LEN + 1];
        assert!(lines.output(&long).is_empty());
        let complete = lines.output(b"\n");
        assert_eq!(complete.first().map(|line| line.0.is_none()), Some(true));
        for line in [
            b"nonsense".as_slice(),
            br#"{"t":"ready","protocol":2}"#,
            br#"{"t":"progress","task":"not-hex","text":"x"}"#,
            br#"{"t":"tool","task":"00000000000000000000000000000001","call":"bad id","name":"x","args":{}}"#,
            br#"{"t":"tool","task":"00000000000000000000000000000001","call":"c1","name":"","args":{}}"#,
        ] {
            assert_eq!(effect(Some(line)), AgentOutputEffect::Ignore);
        }
        // Arguments over the tool bound are a violation, however short the line.
        let text = "x".repeat(ma2a_agent::protocol::MAX_TOOL_ARGS_LEN);
        let huge = format!(
            r#"{{"t":"tool","task":"00000000000000000000000000000001","call":"c1","name":"files.stat","args":{{"text":"{text}"}}}}"#
        );
        assert_eq!(effect(Some(huge.as_bytes())), AgentOutputEffect::Ignore);
        assert_eq!(
            effect(Some(br#"{"t":"ready","protocol":1}"#)),
            AgentOutputEffect::Ready
        );
    }
}
