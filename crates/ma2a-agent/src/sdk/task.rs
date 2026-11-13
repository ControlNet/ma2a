//! The handle of one delivered task.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use super::Shared;
use crate::{
    protocol::{AgentToRuntime, MAX_TOOL_ARGS_LEN, Message, Origin, ReportedState, ToolError},
    tools::Tool,
};

/// Longest progress text the Runtime retains, in bytes.
const MAX_PROGRESS_LEN: usize = 1_024;
/// The error a task reports instead of a message the Runtime would ignore.
const OUT_OF_BOUNDS: &str = "the message exceeds the adapter message bounds";

/// One task this Agent works on.
///
/// Report its end with [`Task::complete`] or [`Task::fail`]. A handle dropped without
/// either reports `cancelled` after a cancellation, and `failed` otherwise.
#[derive(Debug)]
pub struct Task {
    id: String,
    from: Origin,
    input: Message,
    shared: Arc<Shared>,
    messages: mpsc::UnboundedReceiver<Message>,
    cancelled: watch::Receiver<bool>,
    next_call: AtomicU64,
    ended: bool,
}

impl Task {
    pub(super) fn new(
        (id, from, input): (String, Origin, Message),
        shared: Arc<Shared>,
        (messages, cancelled): (mpsc::UnboundedReceiver<Message>, watch::Receiver<bool>),
    ) -> Self {
        Self {
            id,
            from,
            input,
            shared,
            messages,
            cancelled,
            next_call: AtomicU64::new(1),
            ended: false,
        }
    }

    /// Returns the task identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns who created the task.
    pub const fn from(&self) -> &Origin {
        &self.from
    }

    /// Returns the submitted message.
    pub const fn input(&self) -> &Message {
        &self.input
    }

    /// Reports transient progress, cut to 1 KiB at a character boundary.
    pub fn progress(&self, text: &str) {
        let mut end = text.len().min(MAX_PROGRESS_LEN);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        self.shared.emit(&AgentToRuntime::Progress {
            task: self.id.clone(),
            text: text.get(..end).unwrap_or_default().to_owned(),
        });
    }

    /// Adds one Agent message to the conversation; returns whether it was sent.
    ///
    /// A message outside [`Message::is_within_bounds`] is not sent.
    pub fn message(&self, message: Message) -> bool {
        message.is_within_bounds()
            && self.shared.emit(&AgentToRuntime::Message {
                task: self.id.clone(),
                message,
            })
    }

    /// Returns whether the creator asked to cancel.
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }

    /// Completes once the creator asked to cancel.
    pub async fn cancelled(&self) {
        let mut cancelled = self.cancelled.clone();
        let _changed = cancelled.wait_for(|cancelled| *cancelled).await;
    }

    /// Asks the creator for input: the task waits until their next message arrives.
    ///
    /// Returns `None` when the task was cancelled instead. A question outside
    /// [`Message::is_within_bounds`] fails the task and also returns `None`; the task has
    /// then ended, and later reports for it send nothing.
    pub async fn ask(&mut self, question: Message) -> Option<Message> {
        if !question.is_within_bounds() {
            self.report(
                ReportedState::Failed,
                (None, Some(OUT_OF_BOUNDS.to_owned())),
            );
            return None;
        }
        self.report(ReportedState::Waiting, (Some(question), None));
        let mut cancelled = self.cancelled.clone();
        let answer = tokio::select! {
            answer = self.messages.recv() => answer,
            _cancelled = cancelled.wait_for(|cancelled| *cancelled) => None,
        };
        if answer.is_some() {
            self.report(ReportedState::Running, (None, None));
        }
        answer
    }

    /// Calls one tool and waits for its result.
    ///
    /// # Errors
    /// Returns the Runtime's refusal or the tool's failure.
    pub async fn call(
        &self,
        tool: Tool,
        args: &(impl Serialize + Sync),
    ) -> Result<Value, ToolError> {
        let args = serde_json::to_value(args)
            .map_err(|error| ToolError::new("invalid_tool", error.to_string()))?;
        if args.to_string().len() > MAX_TOOL_ARGS_LEN {
            return Err(ToolError::new(
                "too_large",
                "the tool arguments exceed 16 KiB",
            ));
        }
        let call = format!("c{}", self.next_call.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = oneshot::channel();
        match self.shared.tasks().get_mut(&self.id) {
            Some(slot) => slot.calls.insert(call.clone(), answer),
            None => return Err(ToolError::new("task_ended", "the task is no longer held")),
        };
        let sent = self.shared.emit(&AgentToRuntime::Tool {
            task: self.id.clone(),
            call,
            name: tool.name().to_owned(),
            args,
        });
        if !sent {
            return Err(ToolError::new("too_large", "the tool call exceeds a line"));
        }
        answered
            .await
            .unwrap_or_else(|_| Err(ToolError::new("outcome_unknown", "the Agent is stopping")))
    }

    /// Calls one tool with typed arguments and result.
    ///
    /// # Errors
    /// Returns the Runtime's refusal, the tool's failure, or a result of another shape.
    pub async fn call_as<R: DeserializeOwned>(
        &self,
        tool: Tool,
        args: &(impl Serialize + Sync),
    ) -> Result<R, ToolError> {
        let value = self.call(tool, args).await?;
        serde_json::from_value(value)
            .map_err(|error| ToolError::new("invalid_tool", error.to_string()))
    }

    /// Ends the task as completed, with an optional inline result.
    pub fn complete(mut self, result: Option<Message>) {
        self.report(ReportedState::Completed, (result, None));
    }

    /// Ends the task as failed.
    pub fn fail(mut self, error: &str) {
        self.report(ReportedState::Failed, (None, Some(error.to_owned())));
    }

    fn report(
        &mut self,
        state: ReportedState,
        (message, error): (Option<Message>, Option<String>),
    ) {
        if self.ended {
            return;
        }
        // The Runtime ignores a whole report whose message is out of bounds, which would
        // leave a released task running forever.
        if message
            .as_ref()
            .is_some_and(|message| !message.is_within_bounds())
        {
            return self.report(
                ReportedState::Failed,
                (None, Some(OUT_OF_BOUNDS.to_owned())),
            );
        }
        let terminal = matches!(
            state,
            ReportedState::Completed | ReportedState::Failed | ReportedState::Cancelled
        );
        let sent = self.shared.emit(&AgentToRuntime::State {
            task: self.id.clone(),
            state,
            message,
            error,
        });
        // A result too large for one line still ends the task, as a failure.
        if terminal && !sent {
            self.shared.emit(&AgentToRuntime::State {
                task: self.id.clone(),
                state: ReportedState::Failed,
                message: None,
                error: Some("the result exceeds the adapter line bound".to_owned()),
            });
        }
        if terminal {
            self.ended = true;
            self.shared.tasks().remove(&self.id);
        }
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        if self.is_cancelled() {
            self.report(ReportedState::Cancelled, (None, None));
        } else {
            let error = Some("the Agent ended the task without a result".to_owned());
            self.report(ReportedState::Failed, (None, error));
        }
    }
}
