use std::fmt;

use zeroize::Zeroizing;

use super::csrf;

/// Current-time source used for deterministic session expiry.
pub trait Clock: fmt::Debug + Send + Sync {
    /// Returns Unix time in milliseconds.
    fn now_ms(&self) -> i64;
}

/// System clock used outside tests.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0_u128, |duration| duration.as_millis());
        i64::try_from(millis).unwrap_or(i64::MAX)
    }
}

/// Session lifetime and count bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct WebAuthConfig {
    pub(super) max_sessions: u32,
}

impl WebAuthConfig {
    /// Sliding idle expiry in milliseconds.
    pub const IDLE_TIMEOUT_MS: i64 = 15 * 60 * 1_000;
    /// Absolute expiry in milliseconds.
    pub const ABSOLUTE_TIMEOUT_MS: i64 = 8 * 60 * 60 * 1_000;
    const DEFAULT_MAX_SESSIONS: u32 = 32;
}

impl Default for WebAuthConfig {
    fn default() -> Self {
        Self {
            max_sessions: Self::DEFAULT_MAX_SESSIONS,
        }
    }
}

/// CLI-only password lifecycle action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PasswordAction {
    /// Establishes the first verifier.
    Set,
    /// Replaces an existing verifier.
    Reset,
}

/// Authentication denial without secret-bearing detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AuthFailure {
    /// Credentials, bearer, or session state were not accepted.
    Unauthorized,
    /// The requested CLI lifecycle transition conflicts with current state.
    Conflict,
    /// A bounded internal authentication operation failed.
    Internal,
}

impl fmt::Display for AuthFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unauthorized => formatter.write_str("unauthorized"),
            Self::Conflict => formatter.write_str("credential state conflict"),
            Self::Internal => formatter.write_str("authentication operation failed"),
        }
    }
}

impl std::error::Error for AuthFailure {}

impl From<ma2a_store::StoreError> for AuthFailure {
    fn from(_error: ma2a_store::StoreError) -> Self {
        Self::Internal
    }
}

/// Current browser credential state after a CLI mutation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialState {
    pub(super) auth_epoch: u64,
}

impl CredentialState {
    /// Returns the epoch invalidating all earlier sessions.
    #[must_use]
    pub const fn auth_epoch(self) -> u64 {
        self.auth_epoch
    }
}

/// Newly issued session material returned only to the caller.
pub struct LoginSession {
    pub(super) bearer: Zeroizing<String>,
    pub(super) csrf_token: Zeroizing<String>,
}

impl LoginSession {
    /// Returns the cookie bearer value.
    #[must_use]
    pub fn bearer(&self) -> &str {
        self.bearer.as_str()
    }

    /// Returns the per-session CSRF value.
    #[must_use]
    pub fn csrf_token(&self) -> &str {
        self.csrf_token.as_str()
    }
}

impl fmt::Debug for LoginSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LoginSession { bearer: [REDACTED], csrf_token: [REDACTED] }")
    }
}

/// Authenticated session metadata needed by mutation guards.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticatedSession {
    pub(super) bearer_digest: [u8; 32],
    pub(super) csrf_digest: [u8; 32],
}

impl AuthenticatedSession {
    pub(super) const fn bearer_digest(&self) -> &[u8; 32] {
        &self.bearer_digest
    }

    pub(super) fn csrf_matches(&self, token: &str) -> bool {
        csrf::csrf_matches(token, &self.csrf_digest)
    }
}
