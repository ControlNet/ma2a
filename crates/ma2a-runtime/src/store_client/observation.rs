use ma2a_store::EndpointObservationUpdate;
use tokio::sync::oneshot;

use crate::{
    RuntimeClock as _,
    clock::SystemClock,
    error::{RuntimeError, RuntimeErrorKind},
    state::RuntimeStatus,
    store::{StoreClient, StoreCommand, channel_error},
};

impl StoreClient {
    pub(crate) async fn observe(&self, state: &RuntimeStatus) -> Result<u64, RuntimeError> {
        let (reply, response) = oneshot::channel();
        let observation = EndpointObservationUpdate {
            observed_at_ms: SystemClock.now_ms()?,
            ready: state.ready,
            direct_address_count: observation_count(state.endpoint_addr.ip_addrs().count())?,
            relay_address_count: observation_count(state.endpoint_addr.relay_urls().count())?,
            membership_count: observation_count(state.membership_count())?,
        };
        self.send(StoreCommand::Observe { observation, reply })
            .await?;
        response.await.map_err(channel_error)?
    }
}

fn observation_count(value: usize) -> Result<u64, RuntimeError> {
    u64::try_from(value).map_err(|_| RuntimeError::new(RuntimeErrorKind::ObservationOverflow))
}
