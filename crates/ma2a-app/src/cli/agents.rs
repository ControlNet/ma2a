//! Command-line shape of Agents and their tasks.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Agents: hosted by this Runtime, or on another Endpoint with `--endpoint`.
#[derive(Debug, Args)]
pub(crate) struct AgentArgs {
    /// Complete Endpoint ID of the hosting target. Without it, commands act on the Agents
    /// this Runtime hosts, as its local operator.
    #[arg(long, global = true)]
    pub(crate) endpoint: Option<String>,
    #[command(subcommand)]
    pub(crate) command: AgentCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AgentCommand {
    /// Define, or redefine while stopped, one Agent this Runtime hosts.
    Define(DefineArgs),
    /// Remove one stopped Agent's definition.
    Undefine {
        #[arg(value_name = "AGENT")]
        agent: String,
    },
    /// Start one Agent: its instance starts now and restarts when it ends.
    Start {
        #[arg(value_name = "AGENT")]
        agent: String,
    },
    /// Stop one Agent: its unfinished tasks fail and its instance is terminated.
    Stop {
        #[arg(value_name = "AGENT")]
        agent: String,
    },
    /// List this Runtime's Agents, after one name.
    List {
        #[arg(long, value_name = "AGENT")]
        after: Option<String>,
    },
    /// List the Agents another Endpoint lets this one use (needs --endpoint).
    Discover {
        #[arg(long, value_name = "AGENT")]
        after: Option<String>,
    },
    /// Submit one task.
    Submit(SubmitArgs),
    /// List tasks, of one Agent or (locally) of every Agent.
    Tasks {
        #[arg(value_name = "AGENT")]
        agent: Option<String>,
        #[arg(long, value_name = "TASK_ID")]
        after: Option<String>,
    },
    /// Show one task in full.
    Task(TaskTarget),
    /// Print every event after `--after` until the task ends; reconnects while it can.
    Watch {
        #[command(flatten)]
        target: TaskTarget,
        #[arg(long, value_name = "SEQ", default_value_t = 0)]
        after: u32,
    },
    /// Send the next message to a task that is waiting for input.
    Send {
        #[command(flatten)]
        target: TaskTarget,
        #[command(flatten)]
        message: MessageArgs,
        #[arg(long, value_name = "HEX")]
        request_id: Option<String>,
    },
    /// Cancel one task.
    Cancel(TaskTarget),
    /// Download one artifact of a task into a new file (a remote task also needs --agent).
    Artifact {
        #[command(flatten)]
        target: TaskTarget,
        #[arg(value_name = "NAME")]
        name: String,
        /// Local file to create.
        #[arg(long, value_name = "FILE")]
        output: PathBuf,
    },
}

/// One task; a remote task also names its Agent.
#[derive(Debug, Args)]
pub(crate) struct TaskTarget {
    /// The Agent the task belongs to (required with --endpoint).
    #[arg(long, value_name = "AGENT")]
    pub(crate) agent: Option<String>,
    #[arg(value_name = "TASK_ID")]
    pub(crate) task: String,
}

/// One message: text parts, then data parts, in the order given.
#[derive(Debug, Args)]
pub(crate) struct MessageArgs {
    /// A text part; repeatable.
    #[arg(long = "text", value_name = "TEXT")]
    pub(crate) texts: Vec<String>,
    /// A data part holding one JSON value; repeatable.
    #[arg(long = "data", value_name = "JSON")]
    pub(crate) data: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SubmitArgs {
    #[arg(value_name = "AGENT")]
    pub(crate) agent: String,
    #[command(flatten)]
    pub(crate) message: MessageArgs,
    /// Retry an earlier submit: the same request with the same ID creates one task.
    #[arg(long, value_name = "HEX")]
    pub(crate) request_id: Option<String>,
    /// Follow the task until it ends, then print it in full.
    #[arg(long)]
    pub(crate) wait: bool,
    /// With --endpoint: name the submitting Agent. The target records and shows it; it
    /// never authorizes anything.
    #[arg(long, value_name = "AGENT")]
    pub(crate) from_agent: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct DefineArgs {
    #[arg(value_name = "AGENT")]
    pub(crate) agent: String,
    /// What the Agent does, shown to the members that discover it.
    #[arg(long, default_value = "")]
    pub(crate) description: String,
    /// Tool family the Agent may use: files, transfer, exec, process, proxy, agent.
    #[arg(long = "tool", value_name = "FAMILY")]
    pub(crate) tools: Vec<String>,
    /// Most tasks one instance works on at once, 1 to 16.
    #[arg(long, default_value_t = 1)]
    pub(crate) max_tasks: u8,
    /// Absolute working directory; the Agent's workspace by default.
    #[arg(long)]
    pub(crate) cwd: Option<String>,
    /// Environment addition as NAME=VALUE; repeatable.
    #[arg(long = "env", value_name = "NAME=VALUE")]
    pub(crate) env: Vec<String>,
    /// Program and its arguments, passed as typed argv without any shell.
    #[arg(last = true, required = true, value_name = "PROGRAM")]
    pub(crate) argv: Vec<String>,
}
