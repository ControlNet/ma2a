//! A committed join retains its projection work until all completion effects succeed.
use crate::{EnrollmentStage, actor::Actor, error::RuntimeError};

pub(crate) struct PendingEnrollment {
    pub(crate) owners: std::collections::BTreeSet<ma2a_core::EndpointId>,
    pub(crate) stage: EnrollmentStage,
}

impl Actor {
    pub(crate) async fn reconcile_enrollment(&mut self) -> Result<(), RuntimeError> {
        let Some(pending) = self.maintenance.enrollment.as_mut() else {
            return Ok(());
        };
        let owners = pending.owners.clone();
        pending.stage = EnrollmentStage::ControlLookup;
        #[cfg(test)]
        self.maintenance
            .check(crate::actor::maintenance::FaultPoint::EnrollmentCompletion)?;
        self.refresh_control_lookup().await?;
        if let Some(pending) = self.maintenance.enrollment.as_mut() {
            pending.stage = EnrollmentStage::RelayCandidates;
        }
        self.refresh_relay_candidates().await?;
        self.refresh_private_relay_access().await?;
        if let Some(pending) = self.maintenance.enrollment.as_mut() {
            pending.stage = EnrollmentStage::Publications;
        }
        self.refresh_local_control_publications().await?;
        // Scheduling is synchronous. It happens once, only after all projections converge.
        for owner in owners {
            self.schedule_control_round(
                crate::control_sync::ControlRoundTrigger::Explicit(
                    crate::control_sync::ControlRoundScope::peer(owner),
                ),
                None,
            );
        }
        self.maintenance.enrollment = None;
        Ok(())
    }
}
