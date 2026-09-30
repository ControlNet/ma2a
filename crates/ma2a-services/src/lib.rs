//! Target-side implementations and client helpers of the built-in MA2A streaming services.
//!
//! Every function here runs over an already authorized frame stream; nothing in this crate
//! decides who may call it. Hosts are generic over [`tokio::io`] halves so they run unchanged
//! over QUIC streams, the local stream plane, and in-memory test pipes.

#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

pub mod exec;
pub mod files;
pub mod frame_io;
mod fs_error;
mod process;
mod reply;
pub mod report;
pub mod transfer;

pub use exec::serve_exec;
pub use files::serve_files;
pub use frame_io::{Frame, FrameReader, FrameWriter};
pub use report::{OperationEnd, OperationReport};
pub use transfer::{StagingJournal, serve_transfer};
