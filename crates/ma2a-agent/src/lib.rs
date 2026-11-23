//! The Agent adapter of ma2a: the typed protocol an Agent program speaks with its hosting
//! Runtime over stdio, the tool vocabulary, and a small SDK for Agents written in Rust.
//!
//! An Agent is a logical workload hosted by one Runtime. It never holds an identity of its
//! own: its tools run with exactly the hosting Endpoint's authority, and the Runtime is the
//! only party that talks to other Endpoints.

#![forbid(unsafe_code)]

pub mod protocol;
pub mod sdk;
pub mod tools;
