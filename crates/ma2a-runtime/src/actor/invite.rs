use crate::{EnrollmentCreation, EnrollmentError};

use super::Actor;

impl Actor {
    pub(super) async fn create_enrollment_invite(
        &self,
        creation: EnrollmentCreation,
    ) -> Result<ma2a_core::SignedInviteTicket, EnrollmentError> {
        let issued = self
            .clock
            .now_ms()
            .ok()
            .and_then(|value| u64::try_from(value).ok())
            .and_then(|now_ms| creation.issue_at(now_ms).ok())
            .ok_or_else(EnrollmentError::internal)?;
        self.store
            .create_enrollment_invite(
                issued,
                self.state.endpoint_id,
                self.state.endpoint_addr.clone(),
            )
            .await
            .map_err(|_| EnrollmentError::internal())
    }
}
