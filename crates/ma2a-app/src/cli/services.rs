//! Command-line shape of service grants, exposure, operations, and remote services.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// The five service grants an owner can give one Space member.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one independent command-line switch per service grant"
)]
#[derive(Debug, Args)]
pub(crate) struct GrantArgs {
    /// Space name or complete Space ID.
    #[arg(value_name = "SPACE_REF")]
    pub(crate) space: String,
    /// Complete Endpoint ID of the member.
    #[arg(value_name = "ENDPOINT_ID")]
    pub(crate) endpoint: String,
    /// Read files under the roots this Space exposes.
    #[arg(long)]
    pub(crate) files_read: bool,
    /// Change files under the read-write roots this Space exposes.
    #[arg(long)]
    pub(crate) files_write: bool,
    /// Run programs where this Space exposes Exec.
    #[arg(long)]
    pub(crate) exec: bool,
    /// Open terminals where this Space exposes Shell.
    #[arg(long)]
    pub(crate) shell: bool,
    /// Reach TCP destinations this Space's proxy rules allow.
    #[arg(long)]
    pub(crate) proxy: bool,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ServiceCommand {
    /// Consent to serve one service to a Space; grants alone never serve anything.
    Expose {
        #[command(subcommand)]
        exposure: ExposureArgs,
        #[arg(long, global = true)]
        json: bool,
    },
    /// Withdraw one exposure.
    Unexpose {
        #[command(subcommand)]
        exposure: ExposureArgs,
        #[arg(long, global = true)]
        json: bool,
    },
    /// List every exposure this Runtime serves.
    List {
        #[arg(long)]
        json: bool,
    },
}

/// One exposure selector.
#[derive(Debug, Subcommand)]
pub(crate) enum ExposureArgs {
    /// A named directory root; `unexpose` ignores PATH and --read-only.
    Files {
        #[arg(value_name = "SPACE_REF")]
        space: String,
        /// Root name remote paths start with, as in `ROOT:/dir/file`.
        #[arg(value_name = "ROOT")]
        root: String,
        /// Existing local directory (required to expose).
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// Serve the root read-only to this Space.
        #[arg(long)]
        read_only: bool,
    },
    /// One-shot program execution.
    Exec {
        #[arg(value_name = "SPACE_REF")]
        space: String,
    },
    /// Interactive terminals.
    Shell {
        #[arg(value_name = "SPACE_REF")]
        space: String,
    },
    /// TCP destinations: HOST is `*` or one host, PORTS is `N` or `N-M`.
    Proxy {
        #[arg(value_name = "SPACE_REF")]
        space: String,
        #[arg(value_name = "HOST")]
        host: String,
        #[arg(value_name = "PORTS")]
        ports: String,
    },
}

/// Remote Files and Transfer operations; paths are `ROOT:/dir/file`.
#[derive(Debug, Args)]
pub(crate) struct FileArgs {
    /// Complete Endpoint ID of the target.
    #[arg(long, required = true)]
    pub(crate) endpoint: String,
    #[command(subcommand)]
    pub(crate) command: FileCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum FileCommand {
    /// Show one entry's kind, size, and modification time.
    Stat { path: String },
    /// List a directory.
    Ls { path: String },
    /// Create a directory.
    Mkdir {
        path: String,
        /// Create missing parents; an existing directory succeeds.
        #[arg(short, long)]
        parents: bool,
    },
    /// Remove a file, link, or empty directory.
    Rm {
        path: String,
        /// Remove a directory tree without following links.
        #[arg(short, long)]
        recursive: bool,
    },
    /// Rename within one root.
    Mv {
        from: String,
        to: String,
        /// Replace an existing destination.
        #[arg(long)]
        replace: bool,
    },
    /// Print a file to stdout.
    Cat { path: String },
    /// Write stdin (at most 64 KiB) to a file; use `put` for larger content.
    Write {
        path: String,
        /// Replace an existing file.
        #[arg(long)]
        replace: bool,
    },
    /// Download a file with end-to-end BLAKE3 verification.
    Get {
        path: String,
        local: PathBuf,
        /// Continue an interrupted download of the same source.
        #[arg(long)]
        resume: bool,
        /// Replace an existing local file.
        #[arg(long)]
        force: bool,
    },
    /// Upload a file with end-to-end BLAKE3 verification.
    Put {
        local: PathBuf,
        path: String,
        /// Replace an existing remote file.
        #[arg(long)]
        replace: bool,
    },
}

/// Program options shared by Exec and Shell.
#[derive(Debug, Args)]
pub(crate) struct ProgramArgs {
    /// Complete Endpoint ID of the target.
    #[arg(long, required = true)]
    pub(crate) endpoint: String,
    /// Absolute working directory on the target.
    #[arg(long)]
    pub(crate) cwd: Option<String>,
    /// Environment addition as NAME=VALUE; repeatable.
    #[arg(long = "env", value_name = "NAME=VALUE")]
    pub(crate) env: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ExecArgs {
    #[command(flatten)]
    pub(crate) program: ProgramArgs,
    /// Forward stdin to the program.
    #[arg(short = 'i', long)]
    pub(crate) stdin: bool,
    /// Longest run such as 30s, 10m, or 2h.
    #[arg(long, value_name = "DURATION", default_value = "10m")]
    pub(crate) timeout: String,
    /// Program and its arguments, passed as typed argv without any shell.
    #[arg(last = true, required = true, value_name = "PROGRAM")]
    pub(crate) argv: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ShellArgs {
    #[command(flatten)]
    pub(crate) program: ProgramArgs,
    /// Longest session such as 30m or 8h.
    #[arg(long, value_name = "DURATION", default_value = "8h")]
    pub(crate) lifetime: String,
    /// Program and its arguments; the target's default shell when omitted.
    #[arg(last = true, value_name = "PROGRAM")]
    pub(crate) argv: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ProxyCommand {
    /// Relay stdin and stdout to one destination, for example as an SSH `ProxyCommand`.
    Connect {
        #[command(flatten)]
        destination: ProxyDestination,
    },
    /// Accept local TCP connections and relay each to one destination.
    Forward {
        /// Local listen address such as 127.0.0.1:8080.
        #[arg(long, value_name = "ADDRESS")]
        listen: String,
        #[command(flatten)]
        destination: ProxyDestination,
    },
}

#[derive(Debug, Args)]
pub(crate) struct ProxyDestination {
    /// Complete Endpoint ID of the target.
    #[arg(long, required = true)]
    pub(crate) endpoint: String,
    /// Destination host as the target's rule names it.
    #[arg(value_name = "HOST")]
    pub(crate) host: String,
    #[arg(value_name = "PORT")]
    pub(crate) port: u16,
    /// Close after this long without traffic, such as 5m.
    #[arg(long, value_name = "DURATION", default_value = "10m")]
    pub(crate) idle: String,
}
