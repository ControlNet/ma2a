//! Turning supervisor evidence into guarded durable transitions.

use ma2a_core::{
    ProtocolError,
    service::{ProcessId, ProcessReason, ProcessState, ServiceError},
};
use ma2a_services::supervisor::{EndCause, SupervisorStatus, status};
use ma2a_store::{Committed, ProcessRecord, ProcessTransition, StoreError};

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
    /// answers the record as it is. Every answer comes with the revision it was committed or
    /// read at.
    pub(super) async fn step(
        &self,
        id: ProcessId,
        (to, reason): (ProcessState, Option<ProcessReason>),
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
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
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
        let result = self
            .store()
            .processes(move |repository| repository.transition_process(id, transition))
            .await?;
        let answer = match result {
            Ok(committed) => Ok(committed),
            Err(StoreError::Protocol(ProtocolError::CONFLICT)) => found(self.record_at(id).await?),
            Err(StoreError::Protocol(ProtocolError::NOT_FOUND)) => {
                Err(ServiceError::UnknownProcess)
            }
            Err(_) => Err(ServiceError::Unavailable),
        };
        // The reason a termination was asked for is forgotten only once the Process ended, so
        // an end whose commit failed is retried with it.
        let ended = match &answer {
            Ok(committed) => committed.value().state.is_terminal(),
            Err(error) => *error == ServiceError::UnknownProcess,
        };
        if transition.to.is_terminal() && ended {
            self.clear_process_coordination(id);
        }
        answer
    }

    /// Keeps an active withdrawal reason across a failed call until a terminal state commits.
    pub(super) fn keep_terminating(&self, id: ProcessId) {
        if let Some(entry) = self.coordination().terminating.get_mut(&id)
            && entry.reason == ProcessReason::Unauthorized
        {
            entry.keep = true;
        }
    }

    /// Applies what an authenticated supervisor, or its status record, reported.
    pub(super) async fn apply(
        &self,
        id: ProcessId,
        reported: SupervisorStatus,
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
        let result = self.apply_reported(id, reported).await;
        // If a reported termination could not be stored, keep the shared reason until the
        // follower records the terminal state.
        if result.is_err()
            && matches!(
                reported,
                SupervisorStatus::Ended(_, EndCause::Terminated, _)
            )
        {
            self.keep_terminating(id);
        }
        result
    }

    async fn apply_reported(
        &self,
        id: ProcessId,
        reported: SupervisorStatus,
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
        let Ok(current) = found(self.record_at(id).await?) else {
            self.clear_process_coordination(id);
            return Err(ServiceError::UnknownProcess);
        };
        if current.value().state.is_terminal() {
            self.clear_process_coordination(id);
            return Ok(current);
        }
        let requested = self
            .coordination()
            .terminating
            .get(&id)
            .map_or(ProcessReason::Requested, |entry| entry.reason);
        match step_for(current.value().state, reported, requested) {
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
    ) -> Result<Committed<ProcessRecord>, ServiceError> {
        let directory = self.directory(record.id);
        let decided = tokio::task::spawn_blocking(move || status::read(&directory))
            .await
            .map_or(Err(ServiceError::Internal), |recorded| {
                concluded(recorded, record.state)
            });
        match decided {
            Ok(Concluded::Reported(reported)) => self.apply(record.id, reported).await,
            Ok(Concluded::Ended(ended)) => self.step(record.id, ended).await,
            Err(error) => {
                self.keep_terminating(record.id);
                Err(error)
            }
        }
    }
}

/// The record of a read that found the Process, at the revision it was read at.
pub(super) fn found(
    read: Committed<Option<ProcessRecord>>,
) -> Result<Committed<ProcessRecord>, ServiceError> {
    let revision = read.revision();
    read.into_value()
        .map(|record| Committed::new(revision, record))
        .ok_or(ServiceError::UnknownProcess)
}

/// What a provably gone supervisor's status record proves.
#[derive(Debug, PartialEq, Eq)]
enum Concluded {
    /// The supervisor recorded the end.
    Reported(SupervisorStatus),
    /// No end was recorded, so the Process ends with this state and reason.
    Ended((ProcessState, Option<ProcessReason>)),
}

/// Decides from one read of the status record; a read that failed for a reason other than a
/// malformed record proves nothing yet and is retried, never committed as `lost`.
fn concluded(
    recorded: std::io::Result<Option<SupervisorStatus>>,
    state: ProcessState,
) -> Result<Concluded, ServiceError> {
    let recorded = match recorded {
        Ok(recorded) => recorded,
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => None,
        Err(_) => return Err(ServiceError::Unavailable),
    };
    Ok(match recorded {
        Some(reported) if reported.is_final() => Concluded::Reported(reported),
        // `go` is sent only after `starting` was committed: no child can exist.
        _ if state == ProcessState::Created => {
            Concluded::Ended((ProcessState::Failed, Some(ProcessReason::Interrupted)))
        }
        _ => Concluded::Ended((ProcessState::Lost, Some(ProcessReason::SupervisorLost))),
    })
}

#[cfg(test)]
mod tests {
    use ma2a_core::service::{ProcessReason, ProcessState, Termination};
    use ma2a_services::supervisor::{EndCause, SupervisorStatus};

    use super::{Concluded, concluded, step_for};

    #[test]
    fn an_unreadable_status_record_is_retried_rather_than_lost() {
        let exited = SupervisorStatus::Ended(Termination::Exited(3), EndCause::Exited, 9);
        let busy = std::io::Error::from(std::io::ErrorKind::ResourceBusy);
        assert_eq!(
            concluded(Err(busy), ProcessState::Running),
            Err(ma2a_core::service::ServiceError::Unavailable)
        );
        assert_eq!(
            concluded(Ok(Some(exited)), ProcessState::Running),
            Ok(Concluded::Reported(exited))
        );
        let lost = Concluded::Ended((ProcessState::Lost, Some(ProcessReason::SupervisorLost)));
        assert_eq!(concluded(Ok(None), ProcessState::Running), Ok(lost));
        let malformed = std::io::Error::from(std::io::ErrorKind::InvalidData);
        let interrupted =
            Concluded::Ended((ProcessState::Failed, Some(ProcessReason::Interrupted)));
        assert_eq!(
            concluded(Err(malformed), ProcessState::Created),
            Ok(interrupted)
        );
    }

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
