use tokio::time::Duration;

/// Maximum Runtime-wide concurrent active control dials.
pub const CONTROL_DIAL_CONCURRENCY: usize = 8;
/// Maximum transient attempts for one peer in one round.
pub const CONTROL_MAX_ATTEMPTS: usize = 3;
/// Aggregate deadline for one scheduled reconciliation round.
pub const CONTROL_ROUND_DEADLINE: Duration = Duration::from_secs(30);
