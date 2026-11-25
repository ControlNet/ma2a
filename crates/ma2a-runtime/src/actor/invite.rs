use crate::{
    EnrollmentCreation,
    error::{RuntimeError, RuntimeErrorKind},
};

use super::Actor;

impl Actor {
    /// Signs and records one invitation. The Store's refusals, such as a Space this Runtime
    /// does not hold or does not own, come back as they are, so the caller can tell them from
    /// a failed write.
    pub(super) async fn create_enrollment_invite(
        &mut self,
        creation: EnrollmentCreation,
    ) -> Result<ma2a_store::CreatedEnrollmentInvite, RuntimeError> {
        let issued = u64::try_from(self.clock.now_ms()?)
            .ok()
            .and_then(|now_ms| creation.issue_at(now_ms).ok())
            .ok_or_else(|| RuntimeError::new(RuntimeErrorKind::Clock))?;
        let created = self
            .store
            .create_enrollment_invite(
                issued,
                self.state.endpoint_id,
                self.state.endpoint_addr.clone(),
            )
            .await?;
        self.state.revision = created.revision();
        let _receiver_count = self
            .events
            .send(crate::state::RuntimeEvent::memberships_changed(
                created.revision(),
            ));
        Ok(created)
    }
}
