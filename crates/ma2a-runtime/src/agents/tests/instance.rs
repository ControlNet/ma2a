use ma2a_core::service::{AgentId, ProcessId};
use ma2a_store::AgentInstanceRecord;

use super::{keeps, start_request};

fn instance(generation: u64, process_id: Option<ProcessId>) -> AgentInstanceRecord {
    AgentInstanceRecord {
        generation,
        process_id,
        ready: false,
        cursor: 0,
        next_seq: 0,
        acked_seq: 0,
        started_ms: 1_000,
        ended_ms: None,
    }
}

#[test]
fn only_the_live_instance_that_bound_a_process_keeps_it() {
    let process = ProcessId::from_bytes([7; 16]);
    let other = ProcessId::from_bytes([8; 16]);
    assert!(keeps(Some(&instance(3, Some(process))), (3, process)));
    // Undefined meanwhile: no record and so no instance keeps the Process.
    assert!(!keeps(None, (3, process)));
    // Stopped before the bind: the instance never named a Process.
    assert!(!keeps(Some(&instance(3, None)), (3, process)));
    // A later generation, another Process, or a concluded instance does not keep it.
    assert!(!keeps(Some(&instance(4, Some(process))), (3, process)));
    assert!(!keeps(Some(&instance(3, Some(other))), (3, process)));
    let ended = AgentInstanceRecord {
        ended_ms: Some(2_000),
        ..instance(3, Some(process))
    };
    assert!(!keeps(Some(&ended), (3, process)));
}

#[test]
fn instance_starts_are_distinct_per_definition_and_generation()
-> Result<(), Box<dyn std::error::Error>> {
    let agent = AgentId::new("worker")?;
    let first = start_request(&agent, (1_000, 1))?;
    assert_eq!(first, start_request(&agent, (1_000, 1))?);
    assert_ne!(first, start_request(&agent, (1_000, 2))?);
    // The same generation of a later definition is another start.
    assert_ne!(first, start_request(&agent, (2_000, 1))?);
    Ok(())
}
