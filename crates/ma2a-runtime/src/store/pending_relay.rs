use crate::error::{RuntimeError, RuntimeErrorKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RelayExpiryPolicy {
    AtLeast,
    Exact,
}

pub(super) struct PendingRelayPublication {
    pub(super) config: ma2a_net::PrivateRelayProviderConfig,
    pub(super) authorizations: Vec<ma2a_core::SpaceAuthorizationView>,
    pub(super) skip_spaces: std::collections::BTreeSet<ma2a_core::SpaceId>,
    pub(super) advertisements: Vec<ma2a_core::SignedPrivateRelayAdvertisementV1>,
    pub(super) active_spaces: Vec<ma2a_core::SpaceId>,
    pub(super) clock_behind: bool,
    pub(super) issued_at_ms: u64,
    pub(super) expires_at_ms: u64,
}

impl PendingRelayPublication {
    pub(super) fn can_reuse(
        &self,
        desired: (
            &ma2a_net::PrivateRelayProviderConfig,
            &[ma2a_core::SpaceAuthorizationView],
            &std::collections::BTreeSet<ma2a_core::SpaceId>,
        ),
        (now_ms, desired_expires_at_ms, expiry_policy): (u64, u64, RelayExpiryPolicy),
    ) -> Result<bool, RuntimeError> {
        let (config, authorizations, skip_spaces) = desired;
        let (issued_at_ms, expires_at_ms) = self.effective_window()?;
        if now_ms < issued_at_ms {
            return Err(RuntimeError::new(RuntimeErrorKind::ClockBehind));
        }
        let expiry_matches = match expiry_policy {
            RelayExpiryPolicy::AtLeast => desired_expires_at_ms >= expires_at_ms,
            RelayExpiryPolicy::Exact => desired_expires_at_ms == expires_at_ms,
        };
        Ok(self.config == *config
            && self.authorizations == authorizations
            && self.skip_spaces == *skip_spaces
            && expiry_matches
            && crate::relay_publication::is_current(now_ms, issued_at_ms, expires_at_ms))
    }

    pub(super) fn effective_window(&self) -> Result<(u64, u64), RuntimeError> {
        let window =
            self.advertisements
                .first()
                .map_or((self.issued_at_ms, self.expires_at_ms), |signed| {
                    let advertisement = signed.advertisement();
                    (advertisement.issued_at_ms(), advertisement.expires_at_ms())
                });
        if self.advertisements.iter().any(|signed| {
            (
                signed.advertisement().issued_at_ms(),
                signed.advertisement().expires_at_ms(),
            ) != window
        }) {
            return Err(RuntimeError::new(RuntimeErrorKind::Control));
        }
        Ok(window)
    }
}
