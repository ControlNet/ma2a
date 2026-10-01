//! Command-line shape of hosted services: this Runtime's own, and another Endpoint's.

use std::path::PathBuf;

use clap::{Args, Subcommand};

/// Hosted services, flattened into `ma2a service`.
#[derive(Debug, Subcommand)]
pub(crate) enum HostedServiceCommand {
    /// Register (or replace, while stopped) one hosted service from a JSON manifest.
    Register {
        #[arg(value_name = "SERVICE")]
        service: String,
        /// File holding the JSON manifest: description, call limit, and versions.
        #[arg(long, value_name = "FILE")]
        manifest: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Remove one stopped hosted service.
    Unregister {
        #[arg(value_name = "SERVICE")]
        service: String,
        #[arg(long)]
        json: bool,
    },
    /// Start serving one hosted service once its program attaches.
    Start {
        #[arg(value_name = "SERVICE")]
        service: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop one hosted service: running calls are cancelled and its program is told to stop.
    Stop {
        #[arg(value_name = "SERVICE")]
        service: String,
        #[arg(long)]
        json: bool,
    },
    /// List this Runtime's hosted services, their state, and their calls.
    Hosted {
        /// Continue after this service name.
        #[arg(long, value_name = "SERVICE")]
        after: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the services another Endpoint serves this Endpoint, built-in and hosted.
    Discover {
        /// Complete Endpoint ID of the target.
        #[arg(long, required = true)]
        endpoint: String,
        /// Continue after this service name.
        #[arg(long, value_name = "SERVICE")]
        after: Option<String>,
    },
    /// Call one operation of another Endpoint's hosted service.
    Call(CallArgs),
}

/// One hosted-service call: output goes to stdout, then the result; errors go to stderr.
#[derive(Debug, Args)]
pub(crate) struct CallArgs {
    /// Complete Endpoint ID of the target.
    #[arg(long, required = true)]
    pub(crate) endpoint: String,
    #[arg(value_name = "SERVICE")]
    pub(crate) service: String,
    #[arg(value_name = "OPERATION")]
    pub(crate) operation: String,
    /// Exact version to speak; the highest one listing OPERATION when omitted.
    #[arg(long)]
    pub(crate) version: Option<u16>,
    /// Request body as text, at most 8 KiB.
    #[arg(long, value_name = "TEXT", default_value = "")]
    pub(crate) body: String,
    /// Forward stdin as the call's input once the service accepts it.
    #[arg(short = 'i', long)]
    pub(crate) stdin: bool,
    /// Request ID (32 hex characters): retrying the same call with it never acts twice.
    #[arg(long, value_name = "REQUEST_ID")]
    pub(crate) request_id: Option<String>,
}
