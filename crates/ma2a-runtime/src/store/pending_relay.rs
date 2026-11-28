use super::StoreBackend;
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
        if self.config != *config
            || self.authorizations != authorizations
            || self.skip_spaces != *skip_spaces
        {
            return Ok(false);
        }
        let (issued_at_ms, expires_at_ms) = self.effective_window()?;
        let expiry_matches = match expiry_policy {
            RelayExpiryPolicy::AtLeast => desired_expires_at_ms >= expires_at_ms,
            RelayExpiryPolicy::Exact => desired_expires_at_ms == expires_at_ms,
        };
        Ok(expiry_matches
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

impl StoreBackend {
    pub(super) fn reconcile_active_effective_relay_spaces(
        &mut self,
        local_endpoint_id: ma2a_core::EndpointId,
        (config, authorizations): (
            &ma2a_net::PrivateRelayProviderConfig,
            &[ma2a_core::SpaceAuthorizationView],
        ),
    ) -> Result<(u64, bool), RuntimeError> {
        let mut active_spaces = Vec::new();
        for authorization in authorizations {
            let space_id = authorization.space_id();
            if config.served_spaces().contains(&space_id)
                && authorization.allows_private_relay_provider(local_endpoint_id)
                && self
                    .repository
                    .relay_advertisement(space_id, local_endpoint_id)?
                    .is_some_and(|current| {
                        current.is_active()
                            && advertisement_matches_url(&current, config.public_relay_url())
                    })
            {
                active_spaces.push(space_id);
            }
        }
        self.repository
            .reconcile_private_relay_activity(local_endpoint_id, &active_spaces)
            .map_err(Into::into)
    }

    pub(super) fn prepare_pending_relay_publication(
        &mut self,
        (publisher, authorizations): (
            &ma2a_net::PrivateRelayAdvertisementPublisher,
            &[ma2a_core::SpaceAuthorizationView],
        ),
        (issued_at_ms, expires_at_ms, expiry_policy, skip_spaces): (
            u64,
            u64,
            RelayExpiryPolicy,
            &std::collections::BTreeSet<ma2a_core::SpaceId>,
        ),
    ) -> Result<(), RuntimeError> {
        let reuse = self
            .pending_relay_publication
            .as_ref()
            .map_or(Ok(false), |pending| {
                pending.can_reuse(
                    (publisher.config(), authorizations, skip_spaces),
                    (issued_at_ms, expires_at_ms, expiry_policy),
                )
            })?;
        if !reuse {
            // A changed, renewal-due, or future-issued retained batch cannot be
            // reused. Discard it so per-Space durable high-water checks protect
            // already committed rows without blocking eligible Spaces.
            self.pending_relay_publication = None;
            let request = ma2a_net::AdvertisementPublicationRequest::new(
                authorizations,
                issued_at_ms,
                expires_at_ms,
            )
            .skipping_current(skip_spaces);
            let (advertisements, active_spaces, clock_behind) = publisher
                .advertisement_batch(&mut self.repository, request)
                .map_err(RuntimeError::from)?;
            self.pending_relay_publication = Some(PendingRelayPublication {
                config: publisher.config().clone(),
                authorizations: authorizations.to_vec(),
                skip_spaces: skip_spaces.clone(),
                advertisements,
                active_spaces,
                clock_behind,
                issued_at_ms,
                expires_at_ms,
            });
        }
        Ok(())
    }
}

fn advertisement_matches_url(
    current: &ma2a_store::PersistedRelayAdvertisement,
    relay_url: &ma2a_net::RelayUrl,
) -> bool {
    ma2a_core::SignedPrivateRelayAdvertisementV1::parse_canonical_bytes(
        current.signed_advertisement(),
    )
    .is_ok_and(|signed| signed.advertisement().relay_url() == relay_url)
}
