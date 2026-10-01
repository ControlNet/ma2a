//! Command-line shape of durable Processes.

use clap::{Args, Subcommand};

/// Durable Processes: remote with `--endpoint`, this Runtime's own without it.
#[derive(Debug, Args)]
pub(crate) struct ProcessArgs {
    /// Complete Endpoint ID of the hosting target. Without it, `list`, `inspect`,
    /// `terminate`, and `delete` manage the Processes this Runtime hosts.
    #[arg(long, global = true)]
    pub(crate) endpoint: Option<String>,
    #[command(subcommand)]
    pub(crate) command: ProcessCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProcessCommand {
    /// Start one durable Process that outlives this command and the network.
    Start(StartArgs),
    /// List Processes, after one Process ID.
    List {
        #[arg(long, value_name = "PROCESS_ID")]
        after: Option<String>,
    },
    /// Show one Process, its start, and its retained output window.
    Inspect {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
    },
    /// Print retained output to stdout and stderr.
    Logs {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
        /// `start`, `now`, or a cursor printed by an earlier read.
        #[arg(long, default_value = "start")]
        from: String,
        /// Keep following live output until the Process ends or Ctrl-C.
        #[arg(short, long)]
        follow: bool,
    },
    /// Follow live output; Ctrl-C detaches and leaves the Process running.
    Attach {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
        /// `start`, `now`, or a cursor.
        #[arg(long, default_value = "now")]
        from: String,
        /// Also forward stdin; its EOF closes the Process stdin.
        #[arg(short = 'i', long)]
        stdin: bool,
    },
    /// Write stdin to the Process until EOF.
    Stdin {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
        /// Keep the Process stdin open after EOF, for a later write.
        #[arg(long)]
        keep_open: bool,
    },
    /// Send one signal: interrupt, terminate, kill, hangup, quit, user1, or user2.
    Signal {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
        #[arg(value_name = "SIGNAL")]
        signal: String,
    },
    /// Terminate, then kill after the grace period.
    Terminate {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
        /// Grace period such as 500ms or 10s, at most 60s.
        #[arg(long, value_name = "DURATION", default_value = "10s")]
        grace: String,
    },
    /// Archive an ended Process; its output and start are removed.
    Delete {
        #[arg(value_name = "PROCESS_ID")]
        id: String,
    },
}

#[derive(Debug, Args)]
pub(crate) struct StartArgs {
    /// Display name.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Label as KEY=VALUE; repeatable.
    #[arg(long = "label", value_name = "KEY=VALUE")]
    pub(crate) labels: Vec<String>,
    /// Absolute working directory on the target.
    #[arg(long)]
    pub(crate) cwd: Option<String>,
    /// Environment addition as NAME=VALUE; repeatable.
    #[arg(long = "env", value_name = "NAME=VALUE")]
    pub(crate) env: Vec<String>,
    /// Keep the Process stdin open for `stdin` and `attach --stdin`.
    #[arg(long)]
    pub(crate) stdin: bool,
    /// Retained output in bytes, 1 MiB to 16 MiB.
    #[arg(long, value_name = "BYTES")]
    pub(crate) log_limit: Option<u32>,
    /// Retry an earlier start: the same request with the same ID never starts twice.
    #[arg(long, value_name = "HEX")]
    pub(crate) request_id: Option<String>,
    /// Program and its arguments, passed as typed argv without any shell.
    #[arg(last = true, required = true, value_name = "PROGRAM")]
    pub(crate) argv: Vec<String>,
}
