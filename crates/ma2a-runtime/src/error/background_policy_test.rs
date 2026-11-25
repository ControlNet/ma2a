//! Which Runtime failures background maintenance retries or absorbs.
use super::RuntimeError;

#[test]
fn only_temporary_publication_failures_are_retryable() {
    assert!(
        RuntimeError::from(ma2a_net::AddressPublisherError::ObservationUnavailable)
            .is_retryable_background()
    );
    // A host can briefly hold more addresses than a record carries; the next valid
    // observation publishes again, so the Runtime keeps running meanwhile.
    let invalid = RuntimeError::from(ma2a_net::AddressPublisherError::ObservationInvalid(
        ma2a_core::ProtocolError::INVALID_INPUT,
    ));
    assert!(
        crate::actor::Actor::absorb_background("control publication refresh", Err(invalid)).is_ok()
    );
    assert!(
        !RuntimeError::from(ma2a_net::AddressPublisherError::ObservationPoisoned)
            .is_retryable_background()
    );
    // A clock behind the newest issue time is retried once it catches up.
    assert!(
        RuntimeError::from(ma2a_net::AddressPublisherError::ClockRollback)
            .is_retryable_background()
    );
}

#[test]
fn a_refused_remote_request_does_not_stop_the_runtime() {
    // A full Space history refuses an enrollment or departure; only that request fails.
    let refused = RuntimeError::from(ma2a_store::StoreError::SpaceHistoryFull);
    assert!(refused.is_request_refusal());
    assert!(crate::actor::Actor::absorb_background("owner redemption", Err(refused)).is_ok());
    // So does a Space that has no member slot left for the candidate.
    let full = RuntimeError::from(ma2a_store::StoreError::SpaceMembersFull);
    assert!(crate::actor::Actor::absorb_background("owner redemption", Err(full)).is_ok());
    // And a departure past the receipts kept for one removal.
    let full = RuntimeError::from(ma2a_store::StoreError::DepartureReceiptsFull);
    assert!(crate::actor::Actor::absorb_background("owner departure", Err(full)).is_ok());
    let broken = RuntimeError::from(ma2a_store::StoreError::SpaceNotFound);
    assert!(crate::actor::Actor::absorb_background("owner redemption", Err(broken)).is_err());
}
