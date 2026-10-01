//! Turning supervisor evidence into guarded durable transitions.

use ma2a_core::{
    ProtocolError,
    service::{ProcessId, ProcessReason, ProcessState, ServiceError},
};
use ma2a_services::supervisor::{EndCause, SupervisorStatus, status};
use ma2a_store::{ProcessRecord, ProcessTransition, StoreError};

use super::ProcessManager;

/// The step one supervisor status asks for from `current`, if any.
fn step_for(
    current: ProcessState,
    reported: SupervisorStatus,
    requested: ProcessReason,
) -> Option<ProcessTransition> {
    let (to, reason, termination, at_ms) = match reported {
        SupervisorStatus::Awaiting => return None,
        SupervisorStatus::Running(_, started_ms) => {
            if current != ProcessState::Starting {
                return None;
            }
            (ProcessState::Running, None, None, started_ms)
        }
        SupervisorStatus::Ended(termination, EndCause::Exited, at_ms) => {
            (ProcessState::Exited, None, Some(termination), at_ms)
        }
        SupervisorStatus::Ended(termination, EndCause::Terminated, at_ms) => (
            ProcessState::Terminated,
            Some(requested),
            Some(termination),
            at_ms,
        ),
        SupervisorStatus::SpawnFailed(at_ms) => (
            ProcessState::Failed,
            Some(ProcessReason::SpawnFailed),
            None,
            at_ms,
        ),
        SupervisorStatus::NeverStarted(at_ms) => (
            ProcessState::Failed,
            Some(ProcessReason::Interrupted),
            None,
            at_ms,
        ),
    };
    current.permits(to).then_some(ProcessTransition {
        to,
        reason,
        termination,
        at_ms,
    })
}

impl ProcessManager {
    /// Moves one Process without an observed termination; a step another task already took
    /// answers the record as it is.
    pub(super) async fn step(
        &self,
        id: ProcessId,
        (to, reason): (ProcessState, Option<ProcessReason>),
    ) -> Result<ProcessRecord, ServiceError> {
        let transition = ProcessTransition {
            to,
            reason,
            termination: None,
            at_ms: self.now_ms(),
        };
        self.transition(id, transition).await
    }

    async fn transition(
        &self,
        id: ProcessId,
        transition: ProcessTransition,
    ) -> Result<ProcessRecord, ServiceError> {
        let result = self
            .store()
            .processes(move |repository| repository.transition_process(id, transition))
            .await?;
        if transition.to.is_terminal() {
            self.coordination().terminating.remove(&id);
        }
        match result {
            Ok(committed) => Ok(committed.into_value()),
            Err(StoreError::Protocol(ProtocolError::CONFLICT)) => {
                self.record(id).await?.ok_or(ServiceError::UnknownProcess)
            }
            Err(StoreError::Protocol(ProtocolError::NOT_FOUND)) => {
                Err(ServiceError::UnknownProcess)
            }
            Err(_) => Err(ServiceError::Unavailable),
        }
    }

    /// Applies what an authenticated supervisor, or its status record, reported.
    pub(super) async fn apply(
        &self,
        id: ProcessId,
        reported: SupervisorStatus,
    ) -> Result<ProcessRecord, ServiceError> {
        let current = self.record(id).await?.ok_or(ServiceError::UnknownProcess)?;
        let requested = self
            .coordination()
            .terminating
            .get(&id)
            .copied()
            .unwrap_or(ProcessReason::Requested);
        match step_for(current.state, reported, requested) {
            Some(transition) => self.transition(id, transition).await,
            None => Ok(current),
        }
    }

    /// Decides a Process whose supervisor is provably gone, from its status record alone.
    ///
    /// Only a recorded end is an end. A child that may have been spawned without a recorded
    /// end is `lost`: MA2A can no longer prove what happened to it.
    pub(super) async fn conclude(
        &self,
        record: &ProcessRecord,
    ) -> Result<ProcessRecord, ServiceError> {
        let directory = self.directory(record.id);
        let recorded = tokio::task::spawn_blocking(move || status::read(&directory))
            .await
            .map_err(|_| ServiceError::Internal)?
            .ok()
            .flatten();
        match recorded {
            Some(reported) if reported.is_final() => self.apply(record.id, reported).await,
            // `go` is sent only after `starting` was committed: no child can exist.
            _ if record.state == ProcessState::Created => {
                let interrupted = (ProcessState::Failed, Some(ProcessReason::Interrupted));
                self.step(record.id, interrupted).await
            }
            _ => {
                let lost = (ProcessState::Lost, Some(ProcessReason::SupervisorLost));
                self.step(record.id, lost).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{ProcessReason, ProcessState, Termination};
    use ma2a_services::supervisor::{EndCause, SupervisorStatus};

    use super::step_for;

    #[test]
    fn running_is_asserted_only_from_starting() {
        let running = SupervisorStatus::Running(7, 5);
        let requested = ProcessReason::Requested;
        let step = step_for(ProcessState::Starting, running, requested);
        assert_eq!(step.map(|step| step.to), Some(ProcessState::Running));
        assert_eq!(step_for(ProcessState::Created, running, requested), None);
        assert_eq!(step_for(ProcessState::Running, running, requested), None);
        assert_eq!(step_for(ProcessState::Lost, running, requested), None);
    }

    #[test]
    fn ends_keep_their_cause_and_never_leave_a_terminal_state() {
        let exited = SupervisorStatus::Ended(Termination::Exited(0), EndCause::Exited, 9);
        let terminated =
            SupervisorStatus::Ended(Termination::Signaled(15), EndCause::Terminated, 9);
        let unauthorized = ProcessReason::Unauthorized;
        let step = step_for(ProcessState::Running, exited, unauthorized);
        assert_eq!(
            step.map(|step| (step.to, step.reason)),
            Some((ProcessState::Exited, None))
        );
        let step = step_for(ProcessState::Running, terminated, unauthorized);
        assert_eq!(
            step.map(|step| (step.to, step.reason)),
            Some((ProcessState::Terminated, Some(ProcessReason::Unauthorized)))
        );
        let never = SupervisorStatus::NeverStarted(3);
        let step = step_for(ProcessState::Starting, never, unauthorized);
        assert_eq!(
            step.map(|step| (step.to, step.reason)),
            Some((ProcessState::Failed, Some(ProcessReason::Interrupted)))
        );
        for terminal in [
            ProcessState::Exited,
            ProcessState::Lost,
            ProcessState::Failed,
        ] {
            assert_eq!(step_for(terminal, exited, unauthorized), None);
        }
    }
}
