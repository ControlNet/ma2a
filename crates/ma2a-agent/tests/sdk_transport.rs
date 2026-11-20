//! The SDK loop's transport: a failed output ends it with that failure, and the end of input
//! still delivers every line already queued.

use std::time::Duration;

use ma2a_agent::{
    protocol::{AgentToRuntime, Message, Origin, RuntimeToAgent, decode_line, encode_line},
    sdk::{self, Task},
};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn idle(task: Task) {
    // Held, never answered: the end of input must not wait for it.
    std::future::pending::<()>().await;
    drop(task);
}

#[tokio::test]
async fn a_closed_output_ends_the_loop_with_a_failure_while_input_stays_open() -> TestResult {
    // Given: nobody reads what the Agent writes, but its input is still open.
    let (to_agent, agent_input) = tokio::io::duplex(1 << 16);
    let (agent_output, from_agent) = tokio::io::duplex(64);
    drop(from_agent);

    // When
    let served = tokio::spawn(sdk::serve_on((agent_input, agent_output), idle));

    // Then
    let ended = tokio::time::timeout(Duration::from_secs(5), served).await??;
    assert!(ended.is_err(), "{ended:?}");
    drop(to_agent);
    Ok(())
}

#[tokio::test]
async fn the_end_of_input_still_delivers_the_last_acknowledgement() -> TestResult {
    // Given: one delivery, then the end of input, over a narrow output pipe.
    let (mut to_agent, agent_input) = tokio::io::duplex(1 << 16);
    let (agent_output, from_agent) = tokio::io::duplex(8);
    let delivery = RuntimeToAgent::Task {
        seq: 1,
        task: "t".to_owned(),
        from: Origin {
            endpoint: "creator".to_owned(),
            agent: None,
        },
        message: Message::user_text("hello"),
    };
    to_agent.write_all(&encode_line(&delivery)?).await?;
    drop(to_agent);

    // When
    let served = tokio::spawn(sdk::serve_on((agent_input, agent_output), idle));
    let mut lines = Vec::new();
    let mut from_agent = BufReader::new(from_agent);
    loop {
        let mut line = String::new();
        let read = tokio::time::timeout(Duration::from_secs(5), from_agent.read_line(&mut line));
        if read.await?? == 0 {
            break;
        }
        lines.push(decode_line::<AgentToRuntime>(line.trim_end().as_bytes())?);
    }

    // Then
    tokio::time::timeout(Duration::from_secs(5), served).await???;
    assert!(lines.contains(&AgentToRuntime::Ack { seq: 1 }), "{lines:?}");
    Ok(())
}
