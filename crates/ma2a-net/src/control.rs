mod call;
mod cursor;
mod dial;
mod handler;
mod limit;
mod peer_selection;
mod protocol;
mod sync;

pub use call::{ControlAuthorizedCall, ControlCall, ControlResponder};
pub use cursor::cursor_sequence;
pub use dial::{exchange_control_with_retry, retry_delay};
pub use handler::{ControlMetrics, ControlMetricsSnapshot};
pub use peer_selection::select_peer_window;
pub use protocol::{CONTROL_ALPN, ControlClient, ControlRejection};
pub use sync::{CONTROL_DIAL_CONCURRENCY, CONTROL_MAX_ATTEMPTS, CONTROL_ROUND_DEADLINE};

pub(crate) use handler::ControlHandler;

/// Maximum concurrent inbound control streams for one authenticated peer.
pub const CONTROL_PER_PEER_LIMIT: usize = 2;
/// Maximum concurrent inbound control streams across all peers.
pub const CONTROL_GLOBAL_LIMIT: usize = 16;
