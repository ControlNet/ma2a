//! The daemon lifecycle plane: is a daemon there, which one, and please stop.
//!
//! These questions are asked of the daemon *process*, never of the Runtime it
//! hosts. They are answered from a record fixed once at startup, so a Runtime
//! actor that is busy, blocked on a peer, or already stopped can neither delay
//! nor prevent an answer. That separation is the whole point: the moment a
//! caller most needs to ask whether a daemon is alive is the moment its Runtime
//! is stuck, and a probe that queues behind the stuck work answers nothing.
//!
//! The envelope is deliberately not the local API envelope and carries no
//! negotiation of its own, because it must never change shape. A daemon built
//! long before or after its caller still has to be able to say "I am here" and
//! "I am stopping". A daemon that predates this plane replies with the local
//! API's error envelope, which a caller reads as exactly that and falls back to
//! the versioned handshake.

use ma2a_core::EndpointId;
use serde_json::{Value, json};

mod process;

pub use process::ProcessIncarnation;

use super::IpcError;
use crate::api;

/// Shape of the lifecycle envelope. It is fixed for the life of the transport.
const PROTOCOL: u64 = 1;
const ENVELOPE: &str = "ma2a_lifecycle";

/// What a caller can ask of the daemon process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum LifecycleRequest {
    /// Report identity and liveness.
    Ping,
    /// Begin graceful shutdown.
    Stop,
}

impl LifecycleRequest {
    /// Encodes the request as one frame payload.
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        let operation = match self {
            Self::Ping => "ping",
            Self::Stop => "stop",
        };
        json!({ENVELOPE: PROTOCOL, "op": operation})
            .to_string()
            .into_bytes()
    }

    /// Recognises a lifecycle request, or reports that this is ordinary traffic.
    #[must_use]
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(payload).ok()?;
        if value.get(ENVELOPE).and_then(Value::as_u64) != Some(PROTOCOL) {
            return None;
        }
        match value.get("op").and_then(Value::as_str)? {
            "ping" => Some(Self::Ping),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

/// Which boot of which persistent Endpoint a daemon is serving.
///
/// The Endpoint identity survives every restart; the boot identifier does not.
/// Together they say both "this is the same Endpoint you had" and "this is not
/// the same Runtime you were talking to", which is the pair a caller needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeBoot {
    boot_id: [u8; 16],
    endpoint_id: EndpointId,
}

impl RuntimeBoot {
    /// Names one Runtime boot of one persistent Endpoint.
    #[must_use]
    pub const fn new(boot_id: [u8; 16], endpoint_id: EndpointId) -> Self {
        Self {
            boot_id,
            endpoint_id,
        }
    }
}

/// What one daemon launch is, fixed before it begins serving.
///
/// Every field is known once the Runtime has started and the endpoint is bound,
/// and none of them can change while the daemon runs. Holding them as plain data
/// is what lets the lifecycle plane answer without asking anything.
#[derive(Clone, Debug)]
pub struct DaemonIdentity {
    launch_nonce: Option<String>,
    incarnation: ProcessIncarnation,
    boot: RuntimeBoot,
}

impl DaemonIdentity {
    /// Records the identity of the running daemon launch.
    #[must_use]
    pub const fn new(
        launch_nonce: Option<String>,
        incarnation: ProcessIncarnation,
        boot: RuntimeBoot,
    ) -> Self {
        Self {
            launch_nonce,
            incarnation,
            boot,
        }
    }

    /// Records the calling process as the daemon serving one Runtime boot.
    #[must_use]
    pub fn of_current_process(launch_nonce: Option<String>, boot: RuntimeBoot) -> Self {
        Self::new(launch_nonce, ProcessIncarnation::current(), boot)
    }

    /// Returns the nonce of the launch that produced this daemon, if it had one.
    #[must_use]
    pub fn launch_nonce(&self) -> Option<&str> {
        self.launch_nonce.as_deref()
    }

    /// Renders the record as one line for the inherited readiness pipe.
    #[must_use]
    pub fn ready_line(&self) -> String {
        let mut line = self.render("ready").to_string();
        line.push('\n');
        line
    }

    /// Renders the record for the owner-private daemon metadata file.
    #[must_use]
    pub fn metadata(&self) -> String {
        self.render("running").to_string()
    }

    pub(super) fn reply(&self, operation: &str) -> Vec<u8> {
        self.render(operation).to_string().into_bytes()
    }

    fn render(&self, operation: &str) -> Value {
        json!({
            ENVELOPE: PROTOCOL,
            "op": operation,
            "api_version": api::LOCAL_API_VERSION,
            "binary_version": env!("CARGO_PKG_VERSION"),
            "pid": self.incarnation.pid(),
            "process_started_at": self.incarnation.started_at(),
            "launch_nonce": self.launch_nonce,
            "runtime_boot_id": api::encode_hex(&self.boot.boot_id),
            "endpoint_id": api::encode_hex(self.boot.endpoint_id.as_bytes()),
        })
    }
}

/// A daemon's own account of itself, as read from a reply, a readiness line, or
/// the metadata file it left behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DaemonReport {
    operation: String,
    api_version: u64,
    binary_version: String,
    incarnation: ProcessIncarnation,
    launch_nonce: Option<String>,
    runtime_boot_id: String,
    endpoint_id: String,
}

impl DaemonReport {
    /// Reads a record, or reports that these bytes are not one.
    #[must_use]
    pub fn parse(payload: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(payload).ok()?;
        if value.get(ENVELOPE).and_then(Value::as_u64) != Some(PROTOCOL) {
            return None;
        }
        let pid = u32::try_from(value.get("pid").and_then(Value::as_u64)?).ok()?;
        Some(Self {
            operation: value.get("op").and_then(Value::as_str)?.to_owned(),
            api_version: value.get("api_version").and_then(Value::as_u64)?,
            binary_version: value
                .get("binary_version")
                .and_then(Value::as_str)?
                .to_owned(),
            incarnation: ProcessIncarnation::recorded(
                pid,
                value.get("process_started_at").and_then(Value::as_u64),
            ),
            launch_nonce: value
                .get("launch_nonce")
                .and_then(Value::as_str)
                .map(str::to_owned),
            runtime_boot_id: hex_field(&value, "runtime_boot_id", 32)?,
            endpoint_id: hex_field(&value, "endpoint_id", 64)?,
        })
    }

    /// Returns the record kind: `ready`, `running`, `pong`, or `stopping`.
    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    /// Returns the local API version this daemon speaks.
    #[must_use]
    pub const fn api_version(&self) -> u64 {
        self.api_version
    }

    /// Returns the daemon executable's version.
    #[must_use]
    pub fn binary_version(&self) -> &str {
        &self.binary_version
    }

    /// Returns the process this daemon runs as.
    #[must_use]
    pub const fn incarnation(&self) -> ProcessIncarnation {
        self.incarnation
    }

    /// Returns the launch that produced this daemon, if it was launched by one.
    #[must_use]
    pub fn launch_nonce(&self) -> Option<&str> {
        self.launch_nonce.as_deref()
    }

    /// Returns the Runtime boot this daemon is serving.
    #[must_use]
    pub fn runtime_boot_id(&self) -> &str {
        &self.runtime_boot_id
    }

    /// Returns the persistent Endpoint identity this daemon serves.
    #[must_use]
    pub fn endpoint_id(&self) -> &str {
        &self.endpoint_id
    }
}

fn hex_field(value: &Value, field: &str, characters: usize) -> Option<String> {
    let text = value.get(field).and_then(Value::as_str)?;
    (text.len() == characters
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| text.to_owned())
}

/// Produces the random marker that ties a readiness report to one launch.
///
/// # Errors
/// Returns an I/O error when the operating-system random source fails.
pub fn random_launch_nonce() -> Result<String, IpcError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|_| std::io::Error::other("operating-system random source failed"))?;
    Ok(api::encode_hex(&bytes))
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
