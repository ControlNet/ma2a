use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand};

mod secret_argv;

use secret_argv::reject_secret_argv;

#[derive(Debug, Parser)]
#[command(name = "ma2a", version, about = "Endpoint-centric MA2A administration")]
pub(crate) struct Cli {
    #[arg(long, global = true)]
    pub(crate) state_dir: Option<PathBuf>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Start the current state directory's daemon in the background.
    Start,
    /// Gracefully restart the current state directory's daemon.
    Restart,
    /// Gracefully stop the current state directory's daemon.
    Stop,
    Daemon,
    #[command(hide = true)]
    DaemonDetached,
    Status {
        #[arg(long)]
        json: bool,
    },
    Endpoint {
        #[command(subcommand)]
        command: EndpointCommand,
    },
    Space {
        #[command(subcommand)]
        command: SpaceCommand,
    },
    Relay {
        #[command(subcommand)]
        command: RelayCommand,
    },
    Echo(EchoArgs),
    Ui {
        #[command(subcommand)]
        command: UiCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum EndpointCommand {
    Show {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SpaceCommand {
    /// Create a Space with a shared name every member will see.
    Create {
        /// Human-readable Space name; names need not be unique.
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// List the Spaces this Endpoint belongs to.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one Space by name or Space ID.
    Show {
        /// Space name or complete Space ID.
        #[arg(value_name = "SPACE_REF")]
        space: String,
        #[arg(long)]
        json: bool,
    },
    /// Print a single-use invite ticket for a Space to stdout.
    Invite {
        /// Space name or complete Space ID.
        #[arg(value_name = "SPACE_REF")]
        space: String,
        /// Invite lifetime such as 30s or 5m.
        #[arg(long, value_name = "DURATION", default_value = DEFAULT_INVITE_TTL)]
        ttl: String,
    },
    /// Join a Space by reading an invite ticket from the terminal or stdin.
    Accept,
    /// Ask the Space authority to remove this Endpoint from a Space.
    Leave {
        /// Space name or complete Space ID.
        #[arg(value_name = "SPACE_REF")]
        space: String,
        #[arg(long)]
        json: bool,
    },
    /// Administer the membership of a Space this Endpoint owns.
    Member {
        #[command(subcommand)]
        command: MemberCommand,
    },
    /// Inspect or trigger control synchronization for diagnostics.
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
}

/// Invites stay short-lived by default so an unused ticket expires on its own.
pub(crate) const DEFAULT_INVITE_TTL: &str = "5m";

#[derive(Debug, Subcommand)]
pub(crate) enum MemberCommand {
    /// Revoke another Endpoint's membership of a Space this Endpoint owns.
    Remove {
        /// Space name or complete Space ID.
        #[arg(value_name = "SPACE_REF")]
        space: String,
        /// Complete Endpoint ID to remove.
        #[arg(value_name = "ENDPOINT_ID")]
        endpoint: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SyncCommand {
    Status {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        json: bool,
    },
    Now {
        #[arg(long)]
        endpoint: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum RelayCommand {
    Private {
        #[command(subcommand)]
        command: PrivateRelayCommand,
    },
    Public {
        #[command(subcommand)]
        command: PublicRelayCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum PrivateRelayCommand {
    Configure(PrivateRelayArgs),
    Disable,
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct PrivateRelayArgs {
    #[arg(long)]
    pub(crate) listen: String,
    #[arg(long)]
    pub(crate) public_url: String,
    #[arg(long, required = true)]
    pub(crate) serve_space: Vec<String>,
    #[arg(
        long,
        requires = "tls_key",
        conflicts_with = "external_tls",
        required_unless_present = "external_tls"
    )]
    pub(crate) tls_cert: Option<PathBuf>,
    #[arg(long, requires = "tls_cert", conflicts_with = "external_tls")]
    pub(crate) tls_key: Option<PathBuf>,
    #[arg(
        long,
        conflicts_with_all = ["tls_cert", "tls_key"],
        required_unless_present = "tls_cert"
    )]
    pub(crate) external_tls: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PublicRelayCommand {
    Configure {
        #[arg(long)]
        url: String,
    },
    Disable,
    Status {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct EchoArgs {
    #[arg(long, required = true)]
    pub(crate) endpoint: String,
    #[arg(long, required_unless_present = "stdin", conflicts_with = "stdin")]
    pub(crate) text: Option<String>,
    #[arg(long, required_unless_present = "text", conflicts_with = "text")]
    pub(crate) stdin: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum UiCommand {
    /// Set or reset the Web UI password and revoke existing sessions.
    Init,
    /// Revoke all Web UI sessions.
    RevokeAll,
    /// Start or restart Web UI in the daemon and return to the shell.
    Start {
        /// IP address or hostname to bind, including wildcard addresses.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Listener port; 0 asks the OS to select a free port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long)]
        json: bool,
    },
    /// Stop Web UI without stopping the daemon or Endpoint.
    Stop,
    /// Show running/stopped and the current URL.
    Status {
        #[arg(long)]
        json: bool,
    },
}

impl Cli {
    pub(crate) fn parse_safe(arguments: Vec<OsString>) -> Result<Self, clap::Error> {
        reject_secret_argv(&arguments)?;
        let mut argv = Vec::with_capacity(arguments.len() + 1);
        argv.push(OsString::from("ma2a"));
        argv.extend(arguments);
        Self::try_parse_from(argv)
    }
}
