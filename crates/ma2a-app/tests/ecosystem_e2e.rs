//! Phase 5 acceptance: hosted services between real daemons through the installed CLI.
#![cfg(unix)]

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;
#[path = "services_e2e/harness.rs"]
mod harness;
#[path = "ecosystem_e2e/lab.rs"]
mod lab;

#[path = "ecosystem_e2e/acceptance.rs"]
mod acceptance;
#[path = "ecosystem_e2e/recovery.rs"]
mod recovery;
