use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand};

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
    Daemon,
    #[command(hide = true)]
    DaemonDetached,
    Init,
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
    Web,
    Shutdown,
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
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        json: bool,
    },
    Show {
        #[arg(long)]
        space: String,
        #[arg(long)]
        json: bool,
    },
    Invite {
        #[command(subcommand)]
        command: InviteCommand,
    },
    Member {
        #[command(subcommand)]
        command: MemberCommand,
    },
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum InviteCommand {
    Create {
        #[arg(long)]
        space: String,
        #[arg(long)]
        ttl: String,
        #[arg(long, conflicts_with = "stdout")]
        file: Option<PathBuf>,
        #[arg(long, conflicts_with = "file")]
        stdout: bool,
    },
    Redeem(InviteRedeemArgs),
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub(crate) struct InviteRedeemArgs {
    #[arg(long)]
    pub(crate) stdin: bool,
    #[arg(long)]
    pub(crate) file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum MemberCommand {
    Revoke {
        #[arg(long)]
        space: String,
        #[arg(long)]
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
#[group(required = true, multiple = false)]
pub(crate) struct EchoArgs {
    #[arg(long)]
    pub(crate) endpoint: String,
    #[arg(long)]
    pub(crate) text: Option<String>,
    #[arg(long)]
    pub(crate) stdin: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum UiCommand {
    Password {
        #[command(subcommand)]
        command: PasswordCommand,
    },
    Sessions {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    #[command(hide = true)]
    Session {
        #[command(subcommand)]
        command: SessionsCommand,
    },
    Open,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PasswordCommand {
    Set,
    Reset,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionsCommand {
    RevokeAll,
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

fn reject_secret_argv(arguments: &[OsString]) -> Result<(), clap::Error> {
    let values = arguments
        .iter()
        .map(|value| value.to_string_lossy())
        .collect::<Vec<_>>();
    let password_secret = values.iter().any(|value| value == "--password");
    let invite_redeem = values
        .windows(3)
        .any(|window| window == ["space", "invite", "redeem"]);
    let supported_redeem_input = values
        .iter()
        .any(|value| value == "--stdin" || value == "--file");
    if password_secret || (invite_redeem && !supported_redeem_input) {
        Err(clap::Error::raw(
            clap::error::ErrorKind::InvalidValue,
            "secret values are not accepted in argv; use secure input",
        ))
    } else {
        Ok(())
    }
}
