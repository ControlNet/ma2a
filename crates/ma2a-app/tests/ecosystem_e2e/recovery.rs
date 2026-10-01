//! Unavailability and recovery: the service program crashing and returning, the hosting
//! Runtime restarting and crashing at fault points, and the operator stopping the service.

use std::time::Duration;

use serde_json::Value;

use super::{
    daemon_fixture::TestResult,
    harness::{refused, succeed, text},
    lab::{EcoLab, Program, SERVICE, wait_running, wait_state},
};

fn available(lab: &EcoLab) -> super::daemon_fixture::TestValue<Option<bool>> {
    let page = lab.discover()?;
    Ok(page
        .pointer("/services")
        .and_then(Value::as_array)
        .and_then(|services| {
            services
                .iter()
                .find(|service| text(service, "/service") == SERVICE)
        })
        .and_then(|service| service.get("available").and_then(Value::as_bool)))
}

fn echo_works(lab: &EcoLab) -> TestResult {
    let echoed = lab.call(&[SERVICE, "echo", "--body", "back"])?;
    succeed(&echoed)?;
    assert_eq!(echoed.stdout, b"back");
    Ok(())
}

#[test]
fn a_service_recovers_from_crashes_restarts_and_faults_and_stops_cleanly() -> TestResult {
    let mut lab = EcoLab::start("eco-recover")?;

    // The program crashes: the service is unavailable until it attaches again.
    lab.program.take().ok_or("no program")?.kill()?;
    wait_state(&lab.a, "unavailable")?;
    refused(
        &lab.call(&[SERVICE, "echo", "--version", "2"])?,
        "unavailable",
    )?;
    assert_eq!(available(&lab)?, Some(false));
    lab.program = Some(Program::spawn(&lab.a)?);
    wait_state(&lab.a, "ready")?;
    echo_works(&lab)?;

    // A call running when the program dies has an unknown outcome.
    let held = lab.spawn_call(&[SERVICE, "hold"])?;
    wait_running(&lab.a, 1)?;
    lab.program.take().ok_or("no program")?.kill()?;
    refused(&held.wait_with_output()?, "outcome_unknown")?;
    wait_running(&lab.a, 0)?;
    lab.program = Some(Program::spawn(&lab.a)?);
    wait_state(&lab.a, "ready")?;

    // The hosting Runtime restarts: the program attaches again by itself.
    lab.a.restart()?;
    wait_state(&lab.a, "ready")?;
    echo_works(&lab)?;

    // The hosting Runtime crashes after handing a call over, and before relaying its outcome.
    for point in ["after_call_delivered", "before_outcome_relayed"] {
        lab.a.restart_with(&[("MA2A_TEST_SERVICE_FAULT", point)])?;
        wait_state(&lab.a, "ready")?;
        refused(
            &lab.call(&[SERVICE, "echo", "--body", point])?,
            "outcome_unknown",
        )?;
        lab.a.recover_from_fault()?;
        wait_state(&lab.a, "ready")?;
        echo_works(&lab)?;
    }

    // A started service cannot be unregistered; stopping it tells the program to exit.
    refused(&lab.a.run(&["service", "unregister", SERVICE])?, "conflict")?;
    lab.a.run_ok(&["service", "stop", SERVICE])?;
    let status = lab
        .program
        .as_mut()
        .ok_or("no program")?
        .exited(Duration::from_secs(30))?;
    assert!(status.success(), "the stopped program exited with {status}");
    refused(
        &lab.call(&[SERVICE, "echo", "--version", "2"])?,
        "unavailable",
    )?;
    assert_eq!(available(&lab)?, Some(false));
    lab.a.run_ok(&["service", "unregister", SERVICE])?;
    assert_eq!(available(&lab)?, None);
    refused(
        &lab.call(&[SERVICE, "echo", "--version", "2"])?,
        "unknown_service",
    )?;
    lab.shutdown()
}
