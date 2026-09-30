//! Phase 2 acceptance: remote services between real daemons through the installed CLI.
#![cfg(unix)]

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;
#[path = "services_e2e/harness.rs"]
mod harness;

#[path = "services_e2e/authorization.rs"]
mod authorization;
#[path = "services_e2e/files.rs"]
mod files;
#[path = "services_e2e/lifecycle.rs"]
mod lifecycle;
#[path = "services_e2e/process.rs"]
mod process;
#[path = "services_e2e/proxy.rs"]
mod proxy;
