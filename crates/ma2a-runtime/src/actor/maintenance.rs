use ma2a_net::IrohRelayObservation;

#[derive(Default)]
pub(crate) struct Maintenance {
    pub(crate) pending_observation: Option<IrohRelayObservation>,
    pub(crate) address_lookup_pending: bool,
    pub(crate) relay_followup_pending: bool,
    pub(crate) published_relay: Option<PublishedRelay>,
    #[cfg(test)]
    pub(crate) faults:
        std::sync::Arc<std::sync::Mutex<Vec<(FaultPoint, crate::error::RuntimeError)>>>,
    #[cfg(test)]
    pub(crate) candidate_refresh_attempts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    pub(crate) relay_map_apply_attempts: usize,
}

pub(crate) struct PublishedRelay {
    pub(crate) config: ma2a_net::PrivateRelayProviderConfig,
    pub(crate) authorizations: Vec<ma2a_core::SpaceAuthorizationView>,
    pub(crate) issued_at_ms: u64,
    pub(crate) expires_at_ms: u64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FaultPoint {
    RelayCandidateLoad,
    RelayMapApply,
    RelayObservationPersist,
    EndpointObservationPersist,
    AddressPublication,
    RelayPublication,
}

#[cfg(test)]
impl Maintenance {
    pub(crate) fn fail_once(&self, point: FaultPoint, error: crate::error::RuntimeError) {
        self.faults
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((point, error));
    }

    pub(crate) fn check(&self, point: FaultPoint) -> Result<(), crate::error::RuntimeError> {
        let failure = {
            let mut faults = self
                .faults
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            faults
                .iter()
                .position(|(at, _)| *at == point)
                .map(|index| faults.remove(index).1)
        };
        failure.map_or(Ok(()), Err)
    }
}
