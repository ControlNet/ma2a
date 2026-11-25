//! Read-only remote commands give up on a target that opened the stream and fell silent.

use tokio::time::Instant;

use super::READ_DEADLINE;
use crate::{
    AppError,
    cli::{
        agents::{AgentCommand, TaskTarget},
        processes::{ProcessArgs, ProcessCommand},
        services::{FileArgs, FileCommand},
    },
    test_daemon::{Daemon, TestResult},
};

const ID: &str = "0123456789abcdef0123456789abcdef";

/// Runs `command` and checks that its own deadline ended it, as `timed_out`.
async fn times_out(command: impl Future<Output = Result<(), AppError>>) -> TestResult {
    let started = Instant::now();
    let answered = tokio::time::timeout(READ_DEADLINE * 2, command).await;
    assert!(
        matches!(&answered, Ok(Err(AppError::Protocol(message))) if message.starts_with("timed_out")),
        "{answered:?}"
    );
    assert_eq!(started.elapsed(), READ_DEADLINE);
    Ok(())
}

fn task() -> TaskTarget {
    TaskTarget {
        agent: Some("worker".to_owned()),
        task: ID.to_owned(),
    }
}

#[tokio::test(start_paused = true)]
async fn agent_reads_give_up_on_a_silent_target() -> TestResult {
    let target = Daemon::start("agent", Vec::new())?;
    let remote = (&target.paths, target.endpoint.as_str());
    let tasks = AgentCommand::Tasks {
        agent: Some("worker".to_owned()),
        after: None,
    };
    let artifact = AgentCommand::Artifact {
        target: task(),
        name: "result".to_owned(),
        output: target.state_dir.join("artifact"),
    };
    for command in [
        AgentCommand::Discover { after: None },
        tasks,
        AgentCommand::Task(task()),
        artifact,
    ] {
        times_out(Box::pin(super::agent::run(remote, command))).await?;
    }
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn process_reads_give_up_on_a_silent_target() -> TestResult {
    let target = Daemon::start("process", Vec::new())?;
    for command in [
        ProcessCommand::List { after: None },
        ProcessCommand::Inspect { id: ID.to_owned() },
    ] {
        let arguments = ProcessArgs {
            endpoint: Some(target.endpoint.clone()),
            command,
        };
        let listed = super::hosted::run(&target.state_dir, target.paths.clone(), arguments);
        times_out(listed).await?;
    }
    let remote = (&target.paths, target.endpoint.as_str());
    let read = (
        super::hosted::process_id(ID)?,
        ma2a_core::service::LogFrom::Start,
    );
    times_out(super::hosted_stream::logs(remote, read, false)).await?;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_file_download_gives_up_on_a_silent_target() -> TestResult {
    let target = Daemon::start("file", Vec::new())?;
    let remote = (&target.paths, target.endpoint.as_str());
    let local = target.state_dir.join("downloaded");
    let got = super::transfer::get(remote, ("docs:/a", &local), (false, false));
    times_out(Box::pin(got)).await?;
    // The attempt left nothing behind.
    assert!(!local.exists());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_files_operation_gives_up_on_a_silent_target() -> TestResult {
    let target = Daemon::start("files", Vec::new())?;
    let arguments = FileArgs {
        endpoint: target.endpoint.clone(),
        command: FileCommand::Stat {
            path: "docs:/a".to_owned(),
        },
    };
    times_out(Box::pin(super::files::run(target.paths.clone(), arguments))).await?;
    Ok(())
}
