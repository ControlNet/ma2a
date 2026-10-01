//! Closed status set shared by `reject` frames and failed outcomes.

use std::fmt;

use crate::ProtocolError;

macro_rules! service_errors {
    ($($variant:ident = $code:literal, $name:literal, $message:literal;)*) => {
        /// Stable service failure carried by `reject` and failed `outcome` frames.
        #[expect(
            clippy::exhaustive_enums,
            reason = "wire status codes require an exact closed variant set"
        )]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum ServiceError {
            $(
                #[doc = $message]
                $variant,
            )*
        }

        impl ServiceError {
            /// Returns the stable one-byte wire code.
            pub const fn code(self) -> u8 {
                match self {
                    $(Self::$variant => $code,)*
                }
            }

            /// Decodes a stable one-byte wire code.
            pub const fn from_code(code: u8) -> Option<Self> {
                match code {
                    $($code => Some(Self::$variant),)*
                    _ => None,
                }
            }

            /// Returns the stable snake-case name.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)*
                }
            }
        }

        impl fmt::Display for ServiceError {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(match self {
                    $(Self::$variant => $message,)*
                })
            }
        }
    };
}

service_errors! {
    VersionMismatch = 1, "version_mismatch", "service protocol version mismatch";
    InvalidInput = 2, "invalid_input", "invalid service input";
    Unauthorized = 3, "unauthorized", "remote operation is not authorized";
    TimedOut = 4, "timed_out", "service operation timed out";
    ConcurrencyExceeded = 5, "concurrency_exceeded", "service concurrency limit exceeded";
    Cancelled = 6, "cancelled", "service operation was cancelled";
    Unavailable = 7, "unavailable", "service is unavailable";
    Internal = 8, "internal", "internal service failure";
    Conflict = 9, "conflict", "request id was already used for a different request";
    OutcomeUnknown = 10, "outcome_unknown", "a previous attempt with this request id may have taken effect; inspect target state";
    NotFound = 16, "not_found", "path does not exist";
    AlreadyExists = 17, "already_exists", "path already exists";
    NotADirectory = 18, "not_a_directory", "path is not a directory";
    IsADirectory = 19, "is_a_directory", "path is a directory";
    NotEmpty = 20, "not_empty", "directory is not empty";
    SymlinkRefused = 21, "symlink_refused", "symbolic links are not followed";
    InvalidPath = 22, "invalid_path", "path is not valid on the target";
    PermissionDenied = 23, "permission_denied", "the target operating system denied access";
    ReadOnlyRoot = 24, "read_only_root", "root is exported read-only";
    TooLarge = 25, "too_large", "content exceeds the operation bound";
    Io = 26, "io", "target filesystem operation failed";
    IntegrityMismatch = 32, "integrity_mismatch", "content hash or size did not match";
    ContentChanged = 33, "content_changed", "source content changed during transfer";
    PreconditionFailed = 34, "precondition_failed", "resume identity no longer matches";
    SpawnFailed = 48, "spawn_failed", "program could not be started";
    ConnectionRefused = 64, "connection_refused", "destination refused the connection";
    Unreachable = 65, "unreachable", "destination is unreachable";
    DnsFailed = 66, "dns_failed", "destination name could not be resolved";
    UnknownProcess = 80, "unknown_process", "no such Process belongs to this caller";
    NotRunning = 81, "not_running", "the Process is not running";
    WriterBusy = 82, "writer_busy", "another attachment holds the Process stdin";
    StdinClosed = 83, "stdin_closed", "the Process has no open stdin";
    ProcessActive = 84, "process_active", "the Process has not ended yet";
    UnknownAgent = 96, "unknown_agent", "no such Agent is available to this caller";
    UnknownTask = 97, "unknown_task", "no such task of this Agent belongs to this caller";
    AgentUnavailable = 98, "agent_unavailable", "the Agent is not started";
    TaskEnded = 99, "task_ended", "the task has already ended";
    UnknownArtifact = 100, "unknown_artifact", "the task has no artifact of that name";
}

impl ServiceError {
    /// Maps a closed protocol error into the service vocabulary.
    pub fn from_protocol(error: ProtocolError) -> Self {
        if error == ProtocolError::VERSION_MISMATCH {
            Self::VersionMismatch
        } else if error == ProtocolError::UNAUTHORIZED {
            Self::Unauthorized
        } else if error == ProtocolError::UNAVAILABLE {
            Self::Unavailable
        } else if error == ProtocolError::INTERNAL {
            Self::Internal
        } else if error == ProtocolError::NOT_FOUND {
            Self::NotFound
        } else if error == ProtocolError::CONFLICT {
            Self::Conflict
        } else {
            Self::InvalidInput
        }
    }
}

impl std::error::Error for ServiceError {}

#[cfg(test)]
mod tests {
    use super::ServiceError;

    #[test]
    fn every_code_roundtrips_and_unknown_codes_are_rejected() {
        let mut seen = std::collections::BTreeSet::new();
        for code in 0..=u8::MAX {
            if let Some(error) = ServiceError::from_code(code) {
                assert_eq!(error.code(), code);
                assert!(seen.insert(error.name()));
            }
        }
        assert_eq!(seen.len(), 38);
        assert_eq!(ServiceError::from_code(0), None);
    }
}
