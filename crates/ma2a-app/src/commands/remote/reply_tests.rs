//! Remote commands take only the answer the request asked for: another task's, another
//! Agent's, or another Process's answer, or another operation's, fails as a protocol error.

use ma2a_core::{
    RequestId,
    service::{
        AgentAccept, AgentMessage, AgentReply, AgentTaskDetails, AgentTaskId, FrameKind, LogWindow,
        MessagePart, MessageRole, ProcessAccept, ProcessDetails, ProcessId, ProcessReply,
        ProcessSummary,
    },
};

use crate::{
    AppError,
    cli::{
        agents::{AgentCommand, MessageArgs, SubmitArgs, TaskTarget},
        processes::{ProcessArgs, ProcessCommand},
    },
    test_daemon::{Daemon, Script, TestResult, answering, summary},
};

const A: AgentTaskId = AgentTaskId::from_bytes([1; 16]);
const B: AgentTaskId = AgentTaskId::from_bytes([2; 16]);

fn details(task: AgentTaskId, agent: &str) -> TestResult<AgentReply> {
    let input = AgentMessage::new(MessageRole::User, vec![MessagePart::Text("hi".to_owned())])?;
    let details = AgentTaskDetails::new(summary(task, agent)?, input);
    Ok(AgentReply::Details(Box::new(details)))
}

async fn agent(script: Script, command: AgentCommand) -> TestResult<Result<(), AppError>> {
    let target = Daemon::start("reply", script)?;
    let remote = (&target.paths, target.endpoint.as_str());
    Ok(super::agent::run(remote, command).await)
}

fn task_a() -> AgentCommand {
    AgentCommand::Task(TaskTarget {
        agent: Some("worker".to_owned()),
        task: A.to_string(),
    })
}

const fn protocol(result: &Result<(), AppError>) -> bool {
    matches!(result, Err(AppError::Protocol(_)))
}

#[tokio::test]
async fn an_agent_task_read_takes_only_the_task_it_named() -> TestResult {
    let admitted = |reply: TestResult<AgentReply>| answering(AgentAccept::Admitted, &reply?);
    let own = agent(admitted(details(A, "worker"))?, task_a()).await?;
    assert!(own.is_ok(), "{own:?}");
    for (case, reply) in [
        ("another task", details(B, "worker")),
        ("another Agent", details(A, "other")),
        ("a summary", summary(A, "worker").map(AgentReply::Summary)),
    ] {
        let answered = agent(admitted(reply)?, task_a()).await?;
        assert!(protocol(&answered), "{case}: {answered:?}");
    }
    let submitted = answering(AgentAccept::Submitted(A, false), &details(A, "worker")?)?;
    let answered = agent(submitted, task_a()).await?;
    assert!(protocol(&answered), "{answered:?}");
    Ok(())
}

#[tokio::test]
async fn an_agent_submit_takes_only_the_task_it_was_bound_to() -> TestResult {
    let submit = || {
        AgentCommand::Submit(SubmitArgs {
            agent: "worker".to_owned(),
            message: MessageArgs {
                texts: vec!["hi".to_owned()],
                data: Vec::new(),
            },
            request_id: None,
            wait: false,
            from_agent: None,
        })
    };
    let bound = |task, agent: &str| -> TestResult<Script> {
        answering(
            AgentAccept::Submitted(task, false),
            &AgentReply::Summary(summary(A, agent)?),
        )
    };
    let own = agent(bound(A, "worker")?, submit()).await?;
    assert!(own.is_ok(), "{own:?}");
    for script in [bound(B, "worker")?, bound(A, "other")?] {
        let answered = agent(script, submit()).await?;
        assert!(protocol(&answered), "{answered:?}");
    }
    Ok(())
}

#[tokio::test]
async fn an_agent_task_page_lists_only_the_agent_it_named() -> TestResult {
    let tasks = || AgentCommand::Tasks {
        agent: Some("worker".to_owned()),
        after: None,
    };
    let page = |agent: &str| -> TestResult<Script> {
        let page = AgentReply::Page(vec![summary(A, "worker")?, summary(B, agent)?], None);
        answering(AgentAccept::Admitted, &page)
    };
    let own = agent(page("worker")?, tasks()).await?;
    assert!(own.is_ok(), "{own:?}");
    let answered = agent(page("other")?, tasks()).await?;
    assert!(protocol(&answered), "{answered:?}");
    Ok(())
}

fn process_answer(reply: &ProcessReply) -> TestResult<Script> {
    Ok(vec![
        (FrameKind::Accept, ProcessAccept::Admitted.encode()?),
        (
            FrameKind::Outcome,
            ProcessReply::encode_outcome(&Ok(reply.clone()))?,
        ),
    ])
}

fn process_details(id: ProcessId) -> TestResult<ProcessReply> {
    let summary = ProcessSummary::new(id, RequestId::try_from([9; 16].as_slice())?, 1_000);
    let details = ProcessDetails::new(summary, None, LogWindow::new(0, 0));
    Ok(ProcessReply::Details(Box::new(details)))
}

async fn process(script: Script, command: ProcessCommand) -> TestResult<Result<(), AppError>> {
    let target = Daemon::start("reply", script)?;
    let arguments = ProcessArgs {
        endpoint: Some(target.endpoint.clone()),
        command,
    };
    Ok(super::hosted::run(&target.state_dir, target.paths.clone(), arguments).await)
}

#[tokio::test]
async fn a_process_command_takes_only_the_answer_to_its_own_operation() -> TestResult {
    let (a, b) = (
        ProcessId::from_bytes([1; 16]),
        ProcessId::from_bytes([2; 16]),
    );
    let inspect = || ProcessCommand::Inspect { id: a.to_string() };
    let own = process(process_answer(&process_details(a)?)?, inspect()).await?;
    assert!(own.is_ok(), "{own:?}");
    let answered = process(process_answer(&process_details(b)?)?, inspect()).await?;
    assert!(protocol(&answered), "{answered:?}");

    let terminate = ProcessCommand::Terminate {
        id: a.to_string(),
        grace: "1s".to_owned(),
    };
    let answered = process(process_answer(&ProcessReply::Deleted)?, terminate).await?;
    assert!(protocol(&answered), "{answered:?}");
    let delete = ProcessCommand::Delete { id: a.to_string() };
    let answered = process(process_answer(&ProcessReply::Logs(0, true))?, delete).await?;
    assert!(protocol(&answered), "{answered:?}");
    Ok(())
}
