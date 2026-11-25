//! A task handle whose output closed.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicUsize},
};

use serde_json::json;
use tokio::sync::{mpsc, watch};

use super::{Shared, Task};
use crate::{
    protocol::{Message, Origin},
    sdk::Slot,
    tools::Tool,
};

/// A call made once the output writer ended was never sent: it says the Agent is
/// stopping, not that it was too large, and leaves no open call behind.
#[tokio::test]
async fn a_call_the_closed_output_never_sent_leaves_nothing_open() {
    let (out, lines) = mpsc::unbounded_channel();
    drop(lines);
    let shared = Arc::new(Shared {
        out,
        progress: Arc::new(AtomicUsize::new(0)),
        messages: Arc::new(AtomicUsize::new(0)),
        tasks: Mutex::new(HashMap::new()),
    });
    let (inbox, messages) = mpsc::unbounded_channel();
    let (cancel, cancelled) = watch::channel(false);
    let slot = Slot {
        inbox,
        cancel,
        calls: HashMap::new(),
    };
    shared.tasks().insert("a".to_owned(), slot);
    let origin = Origin {
        endpoint: "creator".to_owned(),
        agent: None,
    };
    let input = Message::user_text("tool");
    let task = Task::new(
        ("a".to_owned(), origin, input),
        Arc::clone(&shared),
        (messages, cancelled),
    );
    let answer = task.call(Tool::FilesStat, &json!({"endpoint": "e"})).await;
    assert_eq!(
        answer.map_err(|error| error.code),
        Err("outcome_unknown".to_owned())
    );
    assert!(
        shared
            .tasks()
            .get("a")
            .is_some_and(|slot| slot.calls.is_empty())
    );
}
