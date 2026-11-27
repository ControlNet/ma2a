//! Phase 3 acceptance: durable Processes between real daemons through the installed CLI.
#![cfg(unix)]

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;
#[path = "services_e2e/harness.rs"]
mod harness;
#[path = "process_e2e/lab.rs"]
mod lab;

#[path = "process_e2e/acceptance.rs"]
mod acceptance;
#[path = "process_e2e/adversarial.rs"]
mod adversarial;
#[path = "process_e2e/blocked_pipes.rs"]
mod blocked_pipes;
#[path = "process_e2e/recovery.rs"]
mod recovery;
