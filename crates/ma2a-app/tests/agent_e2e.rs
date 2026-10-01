//! Phase 4 acceptance: Agents between real daemons through the installed CLI.
#![cfg(unix)]

#[path = "support/daemon_fixture.rs"]
mod daemon_fixture;
#[path = "services_e2e/harness.rs"]
mod harness;
#[path = "agent_e2e/lab.rs"]
mod lab;

#[path = "agent_e2e/acceptance.rs"]
mod acceptance;
#[path = "agent_e2e/adversarial.rs"]
mod adversarial;
#[path = "agent_e2e/recovery.rs"]
mod recovery;
